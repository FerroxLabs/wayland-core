//! The Assembler — the pure, deterministic function that turns a task + a keyed
//! candidate pool into a council membership plan, or a single-model Direct.
//!
//! It is a **pure function**: no I/O, no spawning, no clock, no RNG. Given the
//! same `(task, pool, pricing, gate, policy)` it always returns the identical
//! [`AssemblyPlan`], so it is fully snapshot-testable and reproducible.
//!
//! Three orthogonal decisions, in order:
//! 1. **Convene or not** — a `Direct` (Low-stakes) gate answers with one model.
//! 2. **Count + which models** — a complexity ladder ([`member_count`]) ×
//!    maximum provider-family diversity × cheapest-COMPETENT-per-family over the
//!    priced pool. An INTRA-FAMILY price floor keeps a family's flash/mini SKU
//!    from winning that family's slot (it is relative to the family's own
//!    flagship — a family whose only model is a mini still contributes it; an
//!    absolute cross-pool floor is a future refinement).
//! 3. **Aggregator** — decoupled and strong (≥ every proposer), because a judge
//!    weaker than its proposers makes the council worse than one model (the
//!    selection-bottleneck invariant).
//!
//! A budget **downshift ladder** then fits the plan under the stakes-tier cap:
//! step the judge down (never below the strongest proposer), else drop the
//! priciest proposer, else fall to a single strong Direct. Every step is
//! recorded in `trims` so the decision trace is honest.

use std::collections::BTreeMap;

use wcore_pricing::PricingCatalog;
use wcore_types::crucible::MICROCENTS_PER_USD;

use super::gate::{CouncilDecision, Stakes, member_count};
use super::resolver::family;
use super::spend::CouncilSpend;

/// Stage 4c — default cross-vendor Flux roster for empty-state self-bootstrap.
/// Every spec MUST be priceable against the live catalog (an unpriced spec is
/// dropped by the assembler eligibility filter, silently degrading the council),
/// so a hard test below fails the build if any id drifts. Specs are
/// `flux-router:flux-pinned-<model>` and resolve via `flux_pinned_native`.
pub const DEFAULT_FLUX_POOL: &[&str] = &[
    "flux-router:flux-pinned-gpt-5",
    "flux-router:flux-pinned-claude-opus-4-7",
    "flux-router:flux-pinned-deepseek-v4-pro",
    "flux-router:flux-pinned-gemini-2-5-pro",
];

/// Stage 4c — the candidate pool for a `/crucible` run: the configured
/// proposers ∪ candidate_pool, or the [`DEFAULT_FLUX_POOL`] when BOTH are empty
/// (empty-state self-bootstrap so `/crucible` works out-of-the-box on a default
/// config once a Flux key is connected). Returns the raw pool; the caller still
/// runs `resolvable_specs` to filter to keyed specs.
pub fn bootstrap_pool(cfg: &wcore_config::crucible::CrucibleConfig) -> Vec<String> {
    if cfg.proposers.is_empty() && cfg.candidate_pool.is_empty() {
        DEFAULT_FLUX_POOL.iter().map(|s| s.to_string()).collect()
    } else {
        let mut c = cfg.proposers.clone();
        c.extend(cfg.candidate_pool.clone());
        c
    }
}

/// Reference token count used to rank candidate models by a stable unit price.
/// Only relative ordering matters here, so any fixed value works.
const RANK_REF_TOKENS: u64 = 1000;

/// Tunable policy for the Assembler. Built from `[crucible]` config on the auto
/// path; the caps are carried directly (a `fn(Stakes)->f64` pointer could not
/// close over config values).
#[derive(Debug, Clone)]
pub struct AssemblyPolicy {
    /// Families never allowed in a council (operator opt-out, e.g. `--deny`).
    pub deny_families: Vec<String>,
    /// Hard upper bound on proposer count (the blast-radius cap).
    pub max_proposers: usize,
    /// Flux flat-rate / markup factor applied to flux-pinned pricing.
    pub markup: f64,
    /// Judge-inclusive spend caps (USD) by stakes tier.
    pub cap_low_usd: f64,
    pub cap_med_usd: f64,
    pub cap_high_usd: f64,
    /// Intra-family price floor as a fraction of the family's flagship price.
    /// Within a family, models priced below this are dropped as "not competent"
    /// so a flash/mini SKU can't win that family's slot. Relative to the family's
    /// own flagship only (not an absolute cross-pool floor). Clamped to [0,1] at
    /// use. e.g. `0.25`.
    pub price_floor_frac: f64,
    /// Per-proposer turn + token budget the pre-flight estimate prices against
    /// (must match what the council will actually run, so the cap is honest).
    pub proposer_max_turns: usize,
    pub proposer_max_tokens: u32,
}

/// The Assembler's verdict: a council membership, or a single-model Direct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyPlan {
    /// Whether to convene a council. `false` ⇒ run `members[0]` as a single
    /// direct call (or fall back to the caller's default if `members` is empty).
    pub convene: bool,
    /// Proposer specs (when `convene`), or the single Direct model (when not).
    pub members: Vec<String>,
    /// The aggregator spec (only when `convene`).
    pub aggregator: Option<String>,
    /// Judge-inclusive pre-flight estimate (microcents) for the chosen roster.
    /// `None` when the plan is a Direct with no priceable pick.
    pub est_cost_microcents: Option<u64>,
    /// The stakes the gate assigned.
    pub stakes: Stakes,
    /// The real decision trace (families chosen + why), not marketing.
    pub reason: String,
    /// Each downshift step applied to fit the budget, in order.
    pub trims: Vec<String>,
}

/// Split a `provider` / `provider:model` spec into parts (empty model → `None`).
fn spec_parts(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((p, m)) if !m.is_empty() => (p, Some(m)),
        _ => (spec, None),
    }
}

/// A model's unit price at a fixed reference token count — for ranking only.
fn unit_price(catalog: &PricingCatalog, spec: &str, markup: f64) -> Option<u64> {
    let (provider, model) = spec_parts(spec);
    let model = model?;
    catalog.estimate_cost_microcents_resolved(
        provider,
        model,
        RANK_REF_TOKENS,
        RANK_REF_TOKENS,
        markup,
    )
}

/// The judge-inclusive pre-flight cost of a roster (proposers + optional
/// aggregator), in microcents — `None` if any member is unpriceable.
fn roster_cost(
    catalog: &PricingCatalog,
    proposers: &[&str],
    aggregator: Option<&str>,
    policy: &AssemblyPolicy,
) -> Option<u64> {
    let props: Vec<(&str, Option<&str>)> = proposers.iter().map(|s| spec_parts(s)).collect();
    let agg = aggregator.map(spec_parts);
    CouncilSpend::estimate_preflight_microcents(
        catalog,
        &props,
        agg,
        policy.proposer_max_turns,
        policy.proposer_max_tokens,
        policy.markup,
    )
    .certified_microcents()
}

/// The stakes-tier cap in microcents.
fn cap_microcents(policy: &AssemblyPolicy, stakes: Stakes) -> u64 {
    let usd = match stakes {
        Stakes::Low => policy.cap_low_usd,
        Stakes::Med => policy.cap_med_usd,
        Stakes::High => policy.cap_high_usd,
    };
    (usd * MICROCENTS_PER_USD).max(0.0) as u64
}

/// The cheapest priceable single model in the pool (excluding deny-listed
/// families), as a cost-effective Direct pick. Falls back to the first pool spec
/// when nothing is priceable so the caller always has something to run.
fn best_single(
    pool: &[String],
    catalog: &PricingCatalog,
    policy: &AssemblyPolicy,
) -> Option<String> {
    let cheapest = pool
        .iter()
        .filter(|s| !policy.deny_families.iter().any(|d| d == &family(s)))
        .filter_map(|s| unit_price(catalog, s, policy.markup).map(|p| (p, s)))
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)))
        .map(|(_, s)| s.clone());
    cheapest.or_else(|| pool.first().cloned())
}

fn direct_plan(
    members: Vec<String>,
    catalog: &PricingCatalog,
    policy: &AssemblyPolicy,
    stakes: Stakes,
    reason: String,
    trims: Vec<String>,
) -> AssemblyPlan {
    let est = members
        .first()
        .and_then(|s| roster_cost(catalog, &[s.as_str()], None, policy));
    AssemblyPlan {
        convene: false,
        members,
        aggregator: None,
        est_cost_microcents: est,
        stakes,
        reason,
        trims,
    }
}

/// Assemble a council plan (or a Direct) from a candidate pool. Deterministic.
///
/// `pool` MUST already be the runnable, keyed candidate set (run it through
/// [`super::resolver::CouncilProviderResolver::resolvable_specs`] first).
pub fn assemble(
    _task: &str,
    pool: &[String],
    pricing: &PricingCatalog,
    gate: &CouncilDecision,
    policy: &AssemblyPolicy,
) -> AssemblyPlan {
    let stakes = gate.stakes();

    // 1. Direct gate (Low stakes) → answer with one cost-effective model.
    if !gate.is_council() {
        let single = best_single(pool, pricing, policy).into_iter().collect();
        return direct_plan(
            single,
            pricing,
            policy,
            stakes,
            format!("gate routed Direct ({})", gate.reason()),
            vec![],
        );
    }

    // 2. Filter: drop deny-listed families and unpriceable specs.
    let mut excluded: Vec<String> = Vec::new();
    let mut priced: Vec<(String, u64, String)> = Vec::new(); // (spec, unit_price, family)
    for spec in pool {
        let fam = family(spec);
        if policy.deny_families.iter().any(|d| d == &fam) {
            excluded.push(format!("{spec} (deny family '{fam}')"));
            continue;
        }
        match unit_price(pricing, spec, policy.markup) {
            Some(price) => priced.push((spec.clone(), price, fam)),
            None => excluded.push(format!("{spec} (unpriced)")),
        }
    }

    if priced.is_empty() {
        let single = best_single(pool, pricing, policy).into_iter().collect();
        return direct_plan(
            single,
            pricing,
            policy,
            stakes,
            format!(
                "no priceable candidates (excluded: {})",
                excluded.join("; ")
            ),
            vec![],
        );
    }

    // 3. Cheapest-COMPETENT-per-family. Group by family (BTreeMap → deterministic
    //    order); within each family drop the bottom price tier (price < floor ×
    //    family flagship), then take the cheapest survivor as that family's pick.
    let mut by_family: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
    for (spec, price, fam) in &priced {
        by_family
            .entry(fam.clone())
            .or_default()
            .push((spec.clone(), *price));
    }
    let mut candidates: Vec<(String, u64, String)> = Vec::new();
    for (fam, mut models) in by_family {
        let fam_max = models.iter().map(|(_, p)| *p).max().unwrap_or(0);
        // `frac` clamped so a malformed config (>1) can't floor out a family's
        // own flagship and silently drop the family.
        let floor = (fam_max as f64 * policy.price_floor_frac.clamp(0.0, 1.0)) as u64;
        models.retain(|(_, p)| *p >= floor);
        models.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        if let Some((spec, price)) = models.first() {
            candidates.push((spec.clone(), *price, fam));
        }
    }

    // 4. Max diversity: one candidate per family, cheapest first.
    candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let n_families = candidates.len();
    let target_n = member_count(stakes, policy.max_proposers, n_families);

    // Reserve the STRONGEST family as a DECOUPLED judge — it must never also be a
    // proposer, or the council runs no independent adjudicator. So proposers are
    // drawn from at most `n_families - 1` families. A real council needs ≥ 2
    // distinct proposers PLUS a distinct judge (≥ 3 families); anything less is a
    // single strong Direct.
    let cap = cap_microcents(policy, stakes);
    let mut n = target_n.min(n_families.saturating_sub(1));
    if n < 2 {
        let strongest = candidates.last().map(|(s, _, _)| s.clone());
        return direct_plan(
            strongest.into_iter().collect(),
            pricing,
            policy,
            stakes,
            format!(
                "only {n_families} priced family(ies) — too few for a decoupled \
                 council (need ≥3); single strong Direct"
            ),
            vec![],
        );
    }

    // proposers = the `n` cheapest family candidates (indices [0..n)); aggregator
    // = candidates[agg_idx], kept STRICTLY above the proposer band (agg_idx ≥ n)
    // so the judge is always decoupled and ≥ every proposer.
    let mut agg_idx = candidates.len() - 1;
    let mut trims: Vec<String> = Vec::new();

    loop {
        let proposer_specs: Vec<&str> = candidates[0..n]
            .iter()
            .map(|(s, _, _)| s.as_str())
            .collect();
        let agg_spec = candidates[agg_idx].0.as_str();
        let est = roster_cost(pricing, &proposer_specs, Some(agg_spec), policy);

        if let Some(c) = est
            && c <= cap
        {
            let members: Vec<String> = candidates[0..n].iter().map(|(s, _, _)| s.clone()).collect();
            let fams: Vec<&str> = candidates[0..n]
                .iter()
                .map(|(_, _, f)| f.as_str())
                .collect();
            let reason = format!(
                "{stakes:?} council: {n} families [{}] + judge {} (est ${:.4}; \
                 excluded {})",
                fams.join(", "),
                agg_spec,
                c as f64 / MICROCENTS_PER_USD,
                if excluded.is_empty() {
                    "none".to_string()
                } else {
                    excluded.join("; ")
                },
            );
            return AssemblyPlan {
                convene: true,
                members,
                aggregator: Some(agg_spec.to_string()),
                est_cost_microcents: Some(c),
                stakes,
                reason,
                trims,
            };
        }

        // Over cap (or, defensively, unpriceable) → downshift, cheapest lever
        // first. Keep the judge STRICTLY above the proposer band (agg_idx ≥ n) so
        // it stays decoupled and ≥ every proposer.
        if agg_idx > n {
            agg_idx -= 1;
            trims.push(format!("judge↓ to {}", candidates[agg_idx].0));
        } else if n > 2 {
            // Judge is already at its cheapest decoupled slot (agg_idx == n) →
            // drop the priciest proposer; the judge stays decoupled (agg_idx > n
            // now) and a later iteration can step it down again.
            n -= 1;
            trims.push(format!("drop priciest proposer → n={n}"));
        } else {
            // Even a minimal 2-member council is over cap → single strong Direct.
            let strongest = candidates.last().map(|(s, _, _)| s.clone());
            trims.push("council over cap even minimized → single Direct".to_string());
            return direct_plan(
                strongest.into_iter().collect(),
                pricing,
                policy,
                stakes,
                format!(
                    "{stakes:?} council over the ${:.4} cap even minimized",
                    cap as f64 / MICROCENTS_PER_USD
                ),
                trims,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use wcore_pricing::{ModelPrice, PricingCatalog};

    use super::*;

    fn price(input: f64, output: f64) -> ModelPrice {
        ModelPrice {
            input_per_mtok_usd: input,
            output_per_mtok_usd: output,
            cache_read_per_mtok_usd: None,
            cache_write_per_mtok_usd: None,
        }
    }

    /// Six distinct families (a..f), descending flagship price; family `a` also
    /// has a cheap `mini` (bottom-tier) SKU that the price floor must reject.
    fn fixture() -> PricingCatalog {
        let mut providers: HashMap<String, HashMap<String, ModelPrice>> = HashMap::new();
        for (fam, inp, out) in [
            ("a", 10.0, 30.0),
            ("b", 8.0, 24.0),
            ("c", 5.0, 15.0),
            ("d", 3.0, 9.0),
            ("e", 2.0, 6.0),
            ("f", 1.0, 3.0),
        ] {
            let mut models = HashMap::new();
            models.insert("flagship".to_string(), price(inp, out));
            providers.insert(fam.to_string(), models);
        }
        // a:mini — a cheap bottom-tier SKU within family `a`.
        providers
            .get_mut("a")
            .unwrap()
            .insert("mini".to_string(), price(0.3, 0.9));
        PricingCatalog { providers }
    }

    fn pool() -> Vec<String> {
        vec![
            "a:flagship",
            "a:mini",
            "b:flagship",
            "c:flagship",
            "d:flagship",
            "e:flagship",
            "f:flagship",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    fn policy() -> AssemblyPolicy {
        AssemblyPolicy {
            deny_families: vec![],
            max_proposers: 5,
            markup: 1.0,
            cap_low_usd: 0.02,
            cap_med_usd: 0.50,  // generous so the diverse council fits
            cap_high_usd: 1.50, // generous
            price_floor_frac: 0.25,
            proposer_max_turns: 1,
            proposer_max_tokens: 1000,
        }
    }

    fn council(reason: &str) -> CouncilDecision {
        CouncilDecision::Council {
            reason: reason.to_string(),
            stakes: Stakes::Med,
        }
    }

    fn council_high() -> CouncilDecision {
        CouncilDecision::Council {
            reason: "high".to_string(),
            stakes: Stakes::High,
        }
    }

    #[test]
    fn low_stakes_direct_does_not_convene() {
        let gate = CouncilDecision::Direct {
            reason: "trivial".to_string(),
        };
        let plan = assemble("hi", &pool(), &fixture(), &gate, &policy());
        assert!(!plan.convene);
        // A Direct is for a trivial task → the cheapest priceable model overall,
        // which is the bottom-tier a:mini (cost-optimal; the competence floor
        // only applies to COUNCIL proposers, not a trivial Direct answer).
        assert_eq!(plan.members, vec!["a:mini".to_string()]);
        assert!(plan.aggregator.is_none());
    }

    #[test]
    fn med_picks_three_distinct_families_none_bottom_tier() {
        let plan = assemble("design x", &pool(), &fixture(), &council("med"), &policy());
        assert!(plan.convene);
        assert_eq!(plan.members.len(), 3, "Med targets 3 proposers");
        // No bottom-tier SKU (a:mini) is ever a proposer.
        assert!(!plan.members.iter().any(|m| m == "a:mini"));
        // All three proposers are from DISTINCT families.
        let fams: std::collections::HashSet<String> =
            plan.members.iter().map(|m| family(m)).collect();
        assert_eq!(fams.len(), 3, "proposers must span 3 distinct families");
        // Aggregator is the strongest family (a:flagship), decoupled — never a
        // proposer.
        assert_eq!(plan.aggregator.as_deref(), Some("a:flagship"));
        assert!(!plan.members.iter().any(|m| m == "a:flagship"));
        assert!(plan.est_cost_microcents.is_some());
    }

    #[test]
    fn three_families_seat_two_proposers_plus_a_decoupled_judge() {
        // Exactly 3 priced families → 2 proposers + 1 distinct judge (the judge
        // must NOT double as a proposer).
        let three: Vec<String> = ["b:flagship", "c:flagship", "d:flagship"]
            .into_iter()
            .map(String::from)
            .collect();
        let plan = assemble("design x", &three, &fixture(), &council("med"), &policy());
        assert!(plan.convene);
        assert_eq!(plan.members.len(), 2, "3 families → 2 proposers");
        let agg = plan.aggregator.clone().expect("a council has a judge");
        assert!(
            !plan.members.contains(&agg),
            "judge {agg} must be DECOUPLED, not also a proposer"
        );
        assert_eq!(agg, "b:flagship", "judge is the strongest family");
    }

    #[test]
    fn two_families_cannot_seat_a_decoupled_council_so_direct() {
        // 2 families can't seat 2 proposers AND a distinct judge → single Direct.
        let two: Vec<String> = ["b:flagship", "c:flagship"]
            .into_iter()
            .map(String::from)
            .collect();
        let plan = assemble("design x", &two, &fixture(), &council("med"), &policy());
        assert!(!plan.convene, "2 families → no decoupled council → Direct");
    }

    #[test]
    fn high_picks_five_clamped_to_pool() {
        let plan = assemble("audit x", &pool(), &fixture(), &council_high(), &policy());
        assert!(plan.convene);
        assert_eq!(
            plan.members.len(),
            5,
            "High targets 5 (6 families available)"
        );
        let fams: std::collections::HashSet<String> =
            plan.members.iter().map(|m| family(m)).collect();
        assert_eq!(fams.len(), 5);
    }

    #[test]
    fn deny_family_is_excluded() {
        let mut p = policy();
        p.deny_families = vec!["a".to_string()];
        let plan = assemble("design x", &pool(), &fixture(), &council("med"), &p);
        assert!(plan.convene);
        assert!(!plan.members.iter().any(|m| family(m) == "a"));
        assert_ne!(plan.aggregator.as_deref(), Some("a:flagship"));
        // Aggregator falls to the next strongest family (b).
        assert_eq!(plan.aggregator.as_deref(), Some("b:flagship"));
    }

    #[test]
    fn all_unpriced_pool_falls_to_direct() {
        let unpriced: Vec<String> = vec!["x:unknown".to_string(), "y:unknown".to_string()];
        let plan = assemble(
            "design x",
            &unpriced,
            &fixture(),
            &council("med"),
            &policy(),
        );
        assert!(!plan.convene, "no priceable candidates → Direct");
    }

    #[test]
    fn tiny_cap_downshifts_judge_then_falls_to_direct() {
        let mut p = policy();
        p.cap_med_usd = 0.0001; // far below any council
        let plan = assemble("design x", &pool(), &fixture(), &council("med"), &p);
        assert!(!plan.convene, "an impossibly tight cap must fall to Direct");
        assert!(
            !plan.trims.is_empty(),
            "the downshift ladder steps must be recorded"
        );
        assert!(
            plan.trims.iter().any(|t| t.contains("judge↓")),
            "judge is stepped down before falling to Direct"
        );
    }

    #[test]
    fn moderately_tight_cap_downshifts_but_still_convenes() {
        // A cap that the full plan exceeds but a trimmed one fits → still a
        // council, with trims recorded and the est under the cap.
        let mut p = policy();
        p.cap_med_usd = 0.05;
        let plan = assemble("design x", &pool(), &fixture(), &council("med"), &p);
        if plan.convene {
            assert!(!plan.trims.is_empty());
            let cap = (0.05 * MICROCENTS_PER_USD) as u64;
            assert!(plan.est_cost_microcents.unwrap() <= cap);
        }
    }

    #[test]
    fn assembly_is_deterministic() {
        let a = assemble("design x", &pool(), &fixture(), &council("med"), &policy());
        let b = assemble("design x", &pool(), &fixture(), &council("med"), &policy());
        assert_eq!(a, b, "same inputs must produce an identical plan");
    }

    /// Stage 4c — HARD pricing guard for [`DEFAULT_FLUX_POOL`]. An unpriceable
    /// spec is silently dropped by the assembler eligibility filter, degrading
    /// the empty-state council; this test makes a drifted id a BUILD FAILURE.
    /// For each spec: (a) `flux_pinned_native` must resolve a `(provider, model)`,
    /// (b) that resolved pair must price against the live `DEFAULT_CATALOG` via
    /// the same `estimate_cost_microcents_resolved`/`RANK_REF_TOKENS` path the
    /// assembler's `unit_price` uses, and (c) the pool must span ≥2 distinct
    /// vendor families (so the default really is cross-vendor).
    #[test]
    fn default_flux_pool_is_priceable_and_cross_vendor() {
        use std::collections::HashSet;
        use wcore_pricing::{DEFAULT_CATALOG, flux_pinned_native};

        let mut families: HashSet<String> = HashSet::new();
        for spec in DEFAULT_FLUX_POOL {
            // (a) the flux-pinned spec must resolve to a native (provider, model).
            let (provider, model) = flux_pinned_native(spec)
                .unwrap_or_else(|| panic!("DEFAULT_FLUX_POOL spec {spec:?} must flux-resolve"));
            // (b) that resolved native pair must carry a live catalog price.
            assert!(
                DEFAULT_CATALOG
                    .estimate_cost_microcents_resolved(
                        &provider,
                        &model,
                        RANK_REF_TOKENS,
                        RANK_REF_TOKENS,
                        1.0,
                    )
                    .is_some(),
                "DEFAULT_FLUX_POOL spec {spec:?} → ({provider}, {model}) must price \
                 against DEFAULT_CATALOG; an unpriced spec is silently dropped, \
                 degrading the empty-state council"
            );
            // Same path the assembler ranks with — the spec itself must unit-price.
            assert!(
                unit_price(&DEFAULT_CATALOG, spec, 1.0).is_some(),
                "DEFAULT_FLUX_POOL spec {spec:?} must unit_price like the assembler"
            );
            families.insert(family(spec));
        }
        assert!(
            families.len() >= 2,
            "DEFAULT_FLUX_POOL must span ≥2 distinct vendor families (got {families:?})"
        );
    }

    #[test]
    fn bootstrap_pool_uses_default_only_when_both_empty() {
        use wcore_config::crucible::CrucibleConfig;
        // Both empty → the default cross-vendor Flux pool.
        let empty = CrucibleConfig::default();
        assert_eq!(
            bootstrap_pool(&empty),
            DEFAULT_FLUX_POOL
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        // A configured proposer → the default is NOT used (config wins).
        let configured = CrucibleConfig {
            proposers: vec!["openai:gpt-5".to_string()],
            ..CrucibleConfig::default()
        };
        assert_eq!(
            bootstrap_pool(&configured),
            vec!["openai:gpt-5".to_string()]
        );
        // candidate_pool alone also suppresses the default.
        let pooled = CrucibleConfig {
            candidate_pool: vec!["anthropic:claude-opus-4-7".to_string()],
            ..CrucibleConfig::default()
        };
        assert_eq!(
            bootstrap_pool(&pooled),
            vec!["anthropic:claude-opus-4-7".to_string()]
        );
    }

    #[test]
    fn aggregator_is_never_weaker_than_proposers() {
        let plan = assemble("audit x", &pool(), &fixture(), &council_high(), &policy());
        if plan.convene {
            let agg = plan.aggregator.as_deref().unwrap();
            let agg_price = unit_price(&fixture(), agg, 1.0).unwrap();
            for m in &plan.members {
                let mp = unit_price(&fixture(), m, 1.0).unwrap();
                assert!(
                    agg_price >= mp,
                    "judge {agg} ({agg_price}) must be ≥ proposer {m} ({mp})"
                );
            }
        }
    }
}
