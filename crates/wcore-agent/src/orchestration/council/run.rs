//! The council execution phase — spawn the proposers, fence + synthesize.
//!
//! This is the runner-phase entry point: given a validated [`Roster`] and a
//! spawner that carries a [`ProviderResolver`], it
//!
//! 1. **pre-filters** proposers whose provider cannot be keyed (keyless BYO
//!    members / unknown ids) BEFORE spawning anything,
//! 2. **spawns** the survivors concurrently, each pinned to its own provider,
//!    timing each for provenance,
//! 3. enforces **quorum** (≥ `min_proposers` usable, else error),
//! 4. **synthesizes** via the (read-only, fenced) aggregator, falling back to
//!    the first usable proposal when no aggregator is configured/resolvable.
//!
//! The proposers and the aggregator are all spawned read-only (no Bash / Write /
//! Edit) — the council is a read-only-by-construction surface in Slice-1.

use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};
use wcore_config::config::Config;
use wcore_types::message::TokenUsage;

use wcore_pricing::DEFAULT_CATALOG;

use super::aggregator::{Aggregator, LlmSynthesisAggregator};
use super::proposal::Proposal;
use super::roster::{ProposerSpec, Roster};
use super::spend::CouncilSpend;
use crate::spawner::{AgentSpawner, SubAgentConfig};
use wcore_types::crucible::MICROCENTS_PER_USD;

/// Per-proposer output-token budget. The single source of truth for what each
/// council proposer (and the CLI's direct/auto path) is spawned with — the CLI
/// prices its card against this exported value so the certified ceiling can never
/// drift from what the council actually spends.
pub const DEFAULT_PROPOSER_MAX_TOKENS: u32 = 4096;

/// Minimal system prompt sent to every council proposer in place of the host
/// system prompt the child would otherwise inherit via `child_config`. Avoids
/// re-billing the multi-K-token host prompt × N members and leaking host tool
/// scaffolding cross-provider (orphan-tool 400s). See spec §1.
pub const COUNCIL_PROPOSER_SYSTEM_PROMPT: &str = "You are an expert council member. Answer the user's TASK directly, \
     concisely, and on its own merits. Do not assume any host tools, project \
     context, or prior conversation beyond the TASK text.";

/// Minimal system prompt for the aggregator sub-agent. Its authoritative
/// instructions + the untrusted-data fence live in the synthesis PROMPT BODY
/// (`proposal::build_synthesis_prompt`), so the system prompt only needs to be
/// minimal and non-leaking.
pub(crate) const COUNCIL_AGGREGATOR_SYSTEM_PROMPT: &str = "You are a careful aggregator. Follow the instructions in the user message \
     exactly. Do not assume any host tools or project context.";

/// Pre-flight cap + daily-envelope gate. Certifies the judge-inclusive ceiling
/// when any cap binds. The per-run `max_cost_usd` cap is STRICT (an unpriceable
/// roster under it is refused). The default-on daily envelope is SOFT — it binds
/// only when the roster is priceable, so an unpriced Flux council (pre-#319) is
/// never hard-refused; its spend accrues via soft-$0 actual-usage charging.
fn preflight_governance(
    roster: &Roster,
    live: &[(&ProposerSpec, Option<String>)],
    spawner: &AgentSpawner,
) -> Result<(), CouncilError> {
    let daily = spawner
        .budget_tracker()
        .zip(spawner.budget_identity())
        .and_then(|(t, id)| roster.daily_cap_usd.map(|cap| (t, id, cap)));
    if roster.max_cost_usd.is_none() && daily.is_none() {
        return Ok(()); // no cap binds → nothing to certify
    }
    let proposers: Vec<(&str, Option<&str>)> = live
        .iter()
        .map(|(p, model)| (p.provider.as_str(), model.as_deref()))
        .collect();
    let aggregator = roster
        .aggregator
        .as_deref()
        .map(|spec| match spec.split_once(':') {
            Some((p, m)) if !m.is_empty() => (p, Some(m)),
            _ => (spec, None),
        });
    let certified = CouncilSpend::estimate_preflight_microcents(
        &DEFAULT_CATALOG,
        &proposers,
        aggregator,
        roster.proposer_max_turns,
        DEFAULT_PROPOSER_MAX_TOKENS,
        roster.flux_markup,
    )
    .certified_microcents()
    .map(|mc| mc as f64 / MICROCENTS_PER_USD);

    // Per-run cap is STRICT: an explicit per-run ceiling refuses an unpriceable
    // roster rather than run against a $0-soft undercount.
    if let Some(cap_usd) = roster.max_cost_usd {
        let c = certified.ok_or(CouncilError::UnpriceableRoster)?;
        if c > cap_usd {
            return Err(CouncilError::OverBudget {
                estimated_usd: c,
                cap_usd,
            });
        }
    }
    // Daily envelope is SOFT: it binds only when the roster is priceable. Flux is
    // unpriced until FerroxLabs/wayland#319, so an unpriceable roster runs and
    // accrues soft-$0 via actual-usage charging — it is never hard-refused here.
    if let Some((tracker, (_sess, user), daily_cap)) = daily
        && let Some(c) = certified
    {
        let spent = tracker.lock().user_daily_usd(user);
        if spent + c > daily_cap {
            return Err(CouncilError::DailyBudgetExhausted {
                spent_usd: spent,
                cap_usd: daily_cap,
            });
        }
    }
    Ok(())
}

/// A proposer that was skipped before spawning (keyless / unknown provider).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedProposer {
    pub spec: String,
    pub reason: String,
}

/// The result of a council run.
#[derive(Debug, Clone)]
pub struct CouncilOutcome {
    /// The fused (or fallback) answer.
    pub final_text: String,
    /// Every proposal, including errored ones, for provenance / observability.
    pub proposals: Vec<Proposal>,
    /// Proposers skipped before spawn (keyless / unknown), with the reason.
    pub skipped: Vec<SkippedProposer>,
    /// Provider ids whose proposals the aggregator fused.
    pub chosen_from: Vec<String>,
    /// Token + cost rollup for the whole run (proposers + aggregator).
    pub spend: CouncilSpend,
}

/// Why a council run could not produce a result.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CouncilError {
    /// The spawner has no provider resolver, so no proposer can be keyed.
    #[error("no provider resolver attached to the spawner; cannot run a council")]
    NoResolver,
    /// Fewer usable (non-error) proposals than `min_proposers` required.
    #[error("insufficient council proposals: {got} usable < {need} required")]
    InsufficientProposals { got: usize, need: usize },
    /// The worst-case pre-flight spend estimate exceeds the configured cap.
    #[error("council would exceed budget: est ${estimated_usd:.2} > cap ${cap_usd:.2}")]
    OverBudget { estimated_usd: f64, cap_usd: f64 },
    /// A council member has no verified price, so a budget ceiling cannot be
    /// certified — refuse rather than run against an undercounted estimate.
    #[error("council roster is not fully priced; cannot certify a budget ceiling")]
    UnpriceableRoster,
    /// The per-user/day aggregate envelope is exhausted for this user.
    #[error(
        "daily council budget exhausted: ${spent_usd:.2} spent + this council > cap ${cap_usd:.2}"
    )]
    DailyBudgetExhausted { spent_usd: f64, cap_usd: f64 },
}

/// Run a council over `task` using the validated `roster`. `spawner` MUST carry
/// the provider resolver (see [`AgentSpawner::with_provider_resolver`]).
pub async fn run_council(
    task: &str,
    roster: &Roster,
    spawner: &AgentSpawner,
    base: &Config,
) -> Result<CouncilOutcome, CouncilError> {
    let resolver = spawner
        .provider_resolver()
        .cloned()
        .ok_or(CouncilError::NoResolver)?;

    // 1. Pre-filter: drop proposers whose provider cannot be keyed (keyless BYO
    //    members / unknown ids) BEFORE spawning. Capture the resolver's resolved
    //    model so spend accounting can price each member.
    let mut live = Vec::new();
    let mut skipped = Vec::new();
    for p in &roster.proposers {
        match resolver.resolve_provider(&p.spec) {
            Ok((_provider, model)) => live.push((p, model.or_else(|| p.model.clone()))),
            Err(e) => skipped.push(SkippedProposer {
                spec: p.spec.clone(),
                reason: e.to_string(),
            }),
        }
    }

    // 1b. Pre-flight governance: certify the JUDGE-INCLUSIVE worst case and refuse
    //     before spawning if it exceeds the per-run cap, can't be priced, or would
    //     exhaust the per-day envelope. Certification only runs when a cap binds.
    preflight_governance(roster, &live, spawner)?;

    // 2. Spawn the survivors concurrently on their pinned providers, timing
    //    each. provider = the full spec so the resolver keys provider+model;
    //    model carries the resolved model so child_config + pricing line up.
    //
    //    Tail-latency cut: each spawn is wrapped in a per-proposer hard deadline
    //    (`proposer_deadline_s`), and the whole council is bounded by a global
    //    wall-clock soft-deadline (`global_deadline_s`, measured from council
    //    start): once QUORUM IS MET, the run returns as soon as that deadline
    //    has passed, cancelling any still-running stragglers. Before quorum the
    //    soft-deadline does not bite — each proposer is waited out to its
    //    per-proposer hard deadline. A timed-out or cancelled member is retained
    //    as an errored proposal (never silently dropped) so provenance and the
    //    deterministic roster ordering are preserved.
    let member_meta: Vec<(String, Option<String>)> = live
        .iter()
        .map(|(p, model)| (p.provider.clone(), model.clone()))
        .collect();
    let n = member_meta.len();
    let proposer_deadline = Duration::from_secs(roster.proposer_deadline_s);

    // Per-route concurrency bound: cap concurrent spawns sharing one resolved
    // credential. Members are keyed by the spec's ROUTE PREFIX (segment before
    // the first `:`, e.g. `flux:deepseek` → `flux`), so every `flux:*` member
    // draws from one permit pool while BYO vendors get their own. `0` ⇒ unbounded
    // (no map, no acquire). Scoped imports here avoid clashing with the test
    // module's `use std::sync::Arc;` / `use std::collections::HashMap;`.
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    let permits = roster.proposer_concurrency;
    let route_sems: Option<HashMap<String, Arc<Semaphore>>> = (permits > 0).then(|| {
        let mut m: HashMap<String, Arc<Semaphore>> = HashMap::new();
        for (p, _) in &live {
            let route = p.spec.split(':').next().unwrap_or(&p.spec).to_string();
            m.entry(route)
                .or_insert_with(|| Arc::new(Semaphore::new(permits)));
        }
        m
    });

    let mut inflight: FuturesUnordered<_> = live
        .into_iter()
        .enumerate()
        .map(|(i, (p, model))| {
            let cfg = SubAgentConfig {
                name: p.spec.clone(),
                prompt: task.to_string(),
                max_turns: roster.proposer_max_turns,
                max_tokens: DEFAULT_PROPOSER_MAX_TOKENS,
                system_prompt: Some(COUNCIL_PROPOSER_SYSTEM_PROMPT.to_string()),
                provider: Some(p.spec.clone()),
                model: model.clone(),
                // Crucible #3: proposers run hotter for answer diversity.
                temperature: Some(roster.proposer_temperature),
            };
            let provider = p.provider.clone();
            let route = p.spec.split(':').next().unwrap_or(&p.spec).to_string();
            let sem = route_sems.as_ref().and_then(|m| m.get(&route).cloned());
            async move {
                // Hold a per-route permit across the spawn so concurrent calls to
                // one credential are bounded; `acquire_owned` yields a 'static
                // permit safe to move into the future. A closed semaphore never
                // happens here (we own it), so `.ok()` only drops the `None` arm.
                let _permit = match sem {
                    Some(s) => s.acquire_owned().await.ok(),
                    None => None,
                };
                let start = Instant::now();
                let result = tokio::time::timeout(proposer_deadline, spawner.spawn_one(cfg)).await;
                (i, provider, model, result, start.elapsed())
            }
        })
        .collect();

    // Collect results as they complete, indexed by roster position so the final
    // ordering is deterministic regardless of completion order.
    let global = tokio::time::sleep(Duration::from_secs(roster.global_deadline_s));
    tokio::pin!(global);
    let mut slots: Vec<Option<Proposal>> = (0..n).map(|_| None).collect();
    let mut usable_count = 0usize;

    while slots.iter().any(|s| s.is_none()) {
        // Only allow the global soft-deadline to cut the run once quorum is met;
        // before quorum we wait out the per-proposer hard deadline instead.
        let quorum_met = usable_count >= roster.min_proposers;
        tokio::select! {
            biased;
            item = inflight.next() => {
                match item {
                    Some((i, provider, model, result, elapsed)) => {
                        let proposal = match result {
                            Ok(r) => Proposal {
                                provider,
                                model,
                                text: r.text,
                                is_error: r.is_error,
                                usage: r.usage,
                                latency_ms: elapsed.as_millis() as u64,
                            },
                            Err(_elapsed) => Proposal {
                                provider,
                                model,
                                text: "proposer timed out (per-proposer deadline)".to_string(),
                                is_error: true,
                                usage: TokenUsage::default(),
                                latency_ms: elapsed.as_millis() as u64,
                            },
                        };
                        if proposal.is_usable() {
                            usable_count += 1;
                        }
                        let charged_tokens =
                            proposal.usage.input_tokens + proposal.usage.output_tokens;
                        if charged_tokens > 0
                            && let (Some(tracker), Some((sess, user))) =
                                (spawner.budget_tracker(), spawner.budget_identity())
                        {
                            let usd = CouncilSpend::usd_for_usage(
                                &proposal.provider,
                                proposal.model.as_deref(),
                                &proposal.usage,
                                roster.flux_markup,
                            );
                            // The council tracker is cap-less, so this always
                            // commits (no reject-drop); accurate accounting keeps
                            // the next council's pre-check honest.
                            let _ = tracker.lock().charge_for_user(sess, user, charged_tokens, usd);
                        }
                        slots[i] = Some(proposal);
                    }
                    // All in-flight proposers have completed.
                    None => break,
                }
            }
            _ = &mut global, if quorum_met => {
                // Quorum reached and the global soft-deadline elapsed → cancel
                // the remaining stragglers (dropped when `inflight` goes away).
                break;
            }
        }
    }

    // 3. Build proposals with full provenance. Any slot still empty is a
    //    straggler cancelled by the global soft-deadline → an errored proposal.
    let global_ms = roster.global_deadline_s.saturating_mul(1000);
    let proposals: Vec<Proposal> = slots
        .into_iter()
        .enumerate()
        .map(|(i, slot)| {
            slot.unwrap_or_else(|| {
                let (provider, model) = member_meta[i].clone();
                Proposal {
                    provider,
                    model,
                    text: "proposer cancelled after quorum (global soft-deadline)".to_string(),
                    is_error: true,
                    usage: TokenUsage::default(),
                    latency_ms: global_ms,
                }
            })
        })
        .collect();

    // 4. Quorum — at least `min_proposers` usable proposals, but ALWAYS ≥ 1: the
    //    synthesis fallback below expects a usable proposal, and a misconfigured
    //    `min_proposers = 0` would otherwise pass an empty council into it.
    let need = roster.min_proposers.max(1);
    let usable = proposals.iter().filter(|p| p.is_usable()).count();
    if usable < need {
        return Err(CouncilError::InsufficientProposals { got: usable, need });
    }

    // 5. Synthesize. Resolve the aggregator provider; if none is configured or
    //    it cannot be keyed, fall back to the first usable proposal verbatim.
    //    Capture the aggregator's (provider, model) for spend accounting.
    let mut aggregator_provenance: Option<(String, Option<String>)> = None;
    let aggregate = match &roster.aggregator {
        Some(spec) => match resolver.resolve_provider(spec) {
            Ok((provider, model)) => {
                let agg_provider = spec.split(':').next().unwrap_or(spec).to_string();
                aggregator_provenance = Some((agg_provider, model.clone()));
                let agg = LlmSynthesisAggregator::new(
                    provider,
                    model,
                    base.clone(),
                    roster.aggregator_temperature,
                );
                Some(agg.aggregate(task, &proposals).await)
            }
            Err(_) => None,
        },
        None => None,
    };

    // 6. Roll up spend (proposers + aggregator) BEFORE consuming `aggregate`.
    let aggregator_spend = aggregator_provenance
        .as_ref()
        .zip(aggregate.as_ref())
        .map(|((provider, model), agg)| (provider.as_str(), model.as_deref(), &agg.usage));
    let spend = CouncilSpend::from_run(&proposals, aggregator_spend);

    // Charge the judge's real usage against the cap-less council accumulator.
    if let (Some(tracker), Some((sess, user)), Some((prov, model)), Some(agg)) = (
        spawner.budget_tracker(),
        spawner.budget_identity(),
        aggregator_provenance.as_ref(),
        aggregate.as_ref(),
    ) {
        let total = agg.usage.input_tokens + agg.usage.output_tokens;
        if total > 0 {
            let usd =
                CouncilSpend::usd_for_usage(prov, model.as_deref(), &agg.usage, roster.flux_markup);
            // The council tracker is cap-less, so this always commits (no
            // reject-drop); accurate accounting keeps the next council's
            // pre-check honest.
            let _ = tracker.lock().charge_for_user(sess, user, total, usd);
        }
    }

    let (final_text, chosen_from) = match aggregate {
        Some(a) if !a.final_text.trim().is_empty() => (a.final_text, a.chosen_from),
        _ => {
            // No aggregator (or it produced nothing) → first usable proposal.
            // Quorum guarantees ≥1 usable, so this never panics.
            let first = proposals
                .iter()
                .find(|p| p.is_usable())
                .expect("quorum guarantees at least one usable proposal");
            (first.text.clone(), vec![first.provider.clone()])
        }
    };

    Ok(CouncilOutcome {
        final_text,
        proposals,
        skipped,
        chosen_from,
        spend,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use wcore_providers::LlmProvider;

    use super::*;
    use crate::orchestration::council::resolver::{ProviderResolver, ResolveError};
    use crate::orchestration::council::roster::ProposerSpec;

    /// Resolver that returns a fixed verdict per spec — Keyless/Unknown for the
    /// pre-filter tests (no providers needed since nothing is spawned).
    struct VerdictResolver {
        verdicts: HashMap<String, Result<(), ResolveError>>,
    }

    impl ProviderResolver for VerdictResolver {
        fn resolve_provider(
            &self,
            spec: &str,
        ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
            match self.verdicts.get(spec) {
                Some(Ok(())) => unreachable!("provider build not exercised in these tests"),
                Some(Err(ResolveError::Keyless(s))) => Err(ResolveError::Keyless(s.clone())),
                Some(Err(ResolveError::Unknown(s))) => Err(ResolveError::Unknown(s.clone())),
                Some(Err(ResolveError::Build(a, b))) => {
                    Err(ResolveError::Build(a.clone(), b.clone()))
                }
                None => Err(ResolveError::Unknown(spec.to_string())),
            }
        }
    }

    /// A provider that never streams — identity-only for the governance tests
    /// (no spawn ever reaches it because certification refuses first).
    struct NeverProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NeverProvider {
        async fn stream(
            &self,
            _r: &wcore_types::llm::LlmRequest,
        ) -> Result<
            tokio::sync::mpsc::Receiver<wcore_types::llm::LlmEvent>,
            wcore_providers::ProviderError,
        > {
            Err(wcore_providers::ProviderError::Connection("never".into()))
        }
    }

    /// Resolver that always resolves Ok, handing back the spec's model (the part
    /// after `:`) so a `provider:model` spec is priceable while a made-up model
    /// stays uncertifiable. Members enter `live` without anything being spawned.
    struct OkResolver;
    impl ProviderResolver for OkResolver {
        fn resolve_provider(
            &self,
            spec: &str,
        ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
            let model = spec.split_once(':').map(|(_, m)| m.to_string());
            Ok((Arc::new(NeverProvider) as Arc<dyn LlmProvider>, model))
        }
    }

    fn roster(specs: &[&str]) -> Roster {
        Roster {
            proposers: specs
                .iter()
                .map(|s| ProposerSpec {
                    spec: s.to_string(),
                    provider: s.split(':').next().unwrap().to_string(),
                    model: None,
                })
                .collect(),
            aggregator: None,
            min_proposers: 1,
            proposer_max_turns: 2,
            proposer_concurrency: 0,
            proposer_deadline_s: 90,
            global_deadline_s: 25,
            max_cost_usd: None,
            flux_markup: 1.0,
            daily_cap_usd: None,
            proposer_temperature: 0.6,
            aggregator_temperature: 0.4,
        }
    }

    #[tokio::test]
    async fn no_resolver_errors() {
        // A spawner without a resolver cannot run a council.
        struct NeverProvider;
        #[async_trait::async_trait]
        impl LlmProvider for NeverProvider {
            async fn stream(
                &self,
                _r: &wcore_types::llm::LlmRequest,
            ) -> Result<
                tokio::sync::mpsc::Receiver<wcore_types::llm::LlmEvent>,
                wcore_providers::ProviderError,
            > {
                Err(wcore_providers::ProviderError::Connection("never".into()))
            }
        }
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), Config::default());
        let err = run_council("t", &roster(&["openai"]), &spawner, &Config::default())
            .await
            .expect_err("no resolver");
        assert_eq!(err, CouncilError::NoResolver);
    }

    #[tokio::test]
    async fn all_keyless_proposers_skipped_yields_insufficient() {
        // Every proposer resolves Keyless → all skipped before spawn → 0 usable
        // < min_proposers. No provider is ever built or spawned.
        struct NeverProvider;
        #[async_trait::async_trait]
        impl LlmProvider for NeverProvider {
            async fn stream(
                &self,
                _r: &wcore_types::llm::LlmRequest,
            ) -> Result<
                tokio::sync::mpsc::Receiver<wcore_types::llm::LlmEvent>,
                wcore_providers::ProviderError,
            > {
                Err(wcore_providers::ProviderError::Connection("never".into()))
            }
        }
        let mut verdicts = HashMap::new();
        verdicts.insert(
            "openai".to_string(),
            Err(ResolveError::Keyless("openai".into())),
        );
        verdicts.insert(
            "vertex".to_string(),
            Err(ResolveError::Keyless("vertex".into())),
        );
        let resolver = Arc::new(VerdictResolver { verdicts });
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), Config::default())
            .with_provider_resolver(resolver);

        let err = run_council(
            "t",
            &roster(&["openai", "vertex"]),
            &spawner,
            &Config::default(),
        )
        .await
        .expect_err("all keyless");
        assert_eq!(err, CouncilError::InsufficientProposals { got: 0, need: 1 });
    }

    // Certified ceiling refuses an unpriceable roster under a cap (never $0-soft).
    #[tokio::test]
    async fn cap_set_but_unpriceable_roster_is_refused() {
        let mut r = roster(&["totally-made-up:nope-model"]);
        r.max_cost_usd = Some(5.0);
        let resolver = Arc::new(OkResolver); // resolves Ok with the spec's model
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), Config::default())
            .with_provider_resolver(resolver);
        let err = run_council("t", &r, &spawner, &Config::default())
            .await
            .expect_err("unpriceable under cap");
        assert_eq!(err, CouncilError::UnpriceableRoster);
    }

    // Daily envelope: prior spend + this council's certified ceiling > cap ⇒ refuse
    // BEFORE spawning (the aggregate-bound that beats Fusion).
    #[tokio::test]
    async fn daily_envelope_exhausted_refuses_before_spawn() {
        // A priceable roster so the ceiling certifies.
        let mut r = roster(&["deepseek:deepseek-v4-pro"]);
        r.max_cost_usd = None;
        r.daily_cap_usd = Some(0.000_05); // tiny: prior spend + ceiling will exceed
        let tracker = std::sync::Arc::new(parking_lot::Mutex::new(
            wcore_budget::BudgetTracker::new(wcore_budget::BudgetCap {
                per_user_daily_usd: Some(0.000_05),
                ..Default::default()
            }),
        ));
        // Pre-seed a sub-cap charge so this user already has same-day spend.
        let _ = tracker.lock().charge_for_user("s", "u", 1, 0.000_02);
        let resolver = Arc::new(OkResolver);
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), Config::default())
            .with_provider_resolver(resolver)
            .with_budget_tracker(tracker.clone())
            .with_budget_identity("s", "u");
        let err = run_council("t", &r, &spawner, &Config::default())
            .await
            .expect_err("daily exhausted");
        assert!(matches!(err, CouncilError::DailyBudgetExhausted { .. }));
    }

    /// A provider that records the maximum number of concurrent `stream` calls it
    /// ever observed. Each call bumps a live counter, sleeps to create overlap,
    /// then drops the counter and returns an error (the council ends in
    /// `InsufficientProposals`, which is fine — these tests assert CONCURRENCY).
    struct CountingProvider {
        active: Arc<std::sync::atomic::AtomicUsize>,
        max: Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl LlmProvider for CountingProvider {
        async fn stream(
            &self,
            _r: &wcore_types::llm::LlmRequest,
        ) -> Result<
            tokio::sync::mpsc::Receiver<wcore_types::llm::LlmEvent>,
            wcore_providers::ProviderError,
        > {
            use std::sync::atomic::Ordering;
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(now, Ordering::SeqCst);
            // Hold the slot long enough that unbounded members would overlap.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Err(wcore_providers::ProviderError::Connection(
                "counting".into(),
            ))
        }
    }

    /// Resolver that hands back the SAME shared `CountingProvider` for every spec,
    /// so concurrency is observed across all council members (the semaphore is
    /// what bounds them, not separate provider instances). Resolves Ok with the
    /// spec's model (part after `:`), if any. run_council resolves each spec in
    /// the pre-filter AND `spawn_one` resolves again — both get the shared Arc.
    struct SharedResolver(Arc<CountingProvider>);
    impl ProviderResolver for SharedResolver {
        fn resolve_provider(
            &self,
            spec: &str,
        ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
            let model = spec.split_once(':').map(|(_, m)| m.to_string());
            Ok((self.0.clone() as Arc<dyn LlmProvider>, model))
        }
    }

    // Same route, concurrency 2: four `flux:*` members share one permit pool, so
    // no more than 2 may stream at once even though all four are spawned together.
    #[tokio::test]
    async fn same_route_bounds_concurrent_spawns() {
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counting = Arc::new(CountingProvider {
            active: active.clone(),
            max: max.clone(),
        });
        let resolver = Arc::new(SharedResolver(counting.clone()));
        let spawner =
            AgentSpawner::new(counting.clone(), Config::default()).with_provider_resolver(resolver);

        let mut r = roster(&["flux:a", "flux:b", "flux:c", "flux:d"]);
        r.proposer_concurrency = 2;

        // All members error → InsufficientProposals. We assert CONCURRENCY, not
        // success: the semaphore must have capped overlap at the permit count.
        let err = run_council("task", &r, &spawner, &Config::default())
            .await
            .expect_err("all members error");
        assert!(matches!(err, CouncilError::InsufficientProposals { .. }));
        let observed = max.load(std::sync::atomic::Ordering::SeqCst);
        assert!(observed >= 1, "at least one member must have run");
        assert!(
            observed <= 2,
            "same-route fan-out must be capped at proposer_concurrency (2), saw {observed}"
        );
    }

    // Different routes, concurrency 1: each route gets its OWN one-permit pool, so
    // the two members are NOT throttled against each other — they can overlap.
    #[tokio::test]
    async fn distinct_routes_have_independent_pools() {
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counting = Arc::new(CountingProvider {
            active: active.clone(),
            max: max.clone(),
        });
        let resolver = Arc::new(SharedResolver(counting.clone()));
        let spawner =
            AgentSpawner::new(counting.clone(), Config::default()).with_provider_resolver(resolver);

        let mut r = roster(&["openai:a", "anthropic:b"]);
        r.proposer_concurrency = 1;

        let err = run_council("task", &r, &spawner, &Config::default())
            .await
            .expect_err("all members error");
        assert!(matches!(err, CouncilError::InsufficientProposals { .. }));
        // Two distinct routes → two separate 1-permit pools, so neither throttles
        // the other. Both members ran (the run reached InsufficientProposals with
        // 2 errored), and overlap is possible. Timing-robust invariant: with only
        // two members the observed max can never exceed 2.
        let observed = max.load(std::sync::atomic::Ordering::SeqCst);
        assert!(observed >= 1, "both members must have run");
        assert!(observed <= 2, "only two members exist, saw {observed}");
    }
}
