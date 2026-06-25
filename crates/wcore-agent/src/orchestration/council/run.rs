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

use super::aggregator::{Aggregator, LlmSynthesisAggregator};
use super::proposal::Proposal;
use super::roster::Roster;
use super::spend::{CouncilSpend, MICROCENTS_PER_USD};
use crate::spawner::{AgentSpawner, SubAgentConfig};

/// Per-proposer output-token budget.
const DEFAULT_PROPOSER_MAX_TOKENS: u32 = 4096;

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

    // 1b. Pre-flight budget cap: refuse BEFORE spawning if the worst-case spend
    //     would exceed the configured ceiling (a council is N× one call's cost).
    if let Some(cap_usd) = roster.max_cost_usd {
        let members: Vec<(&str, Option<&str>)> = live
            .iter()
            .map(|(p, model)| (p.provider.as_str(), model.as_deref()))
            .collect();
        let est = CouncilSpend::estimate_worst_case_microcents(
            &members,
            roster.proposer_max_turns,
            DEFAULT_PROPOSER_MAX_TOKENS,
        );
        let estimated_usd = est as f64 / MICROCENTS_PER_USD;
        if estimated_usd > cap_usd {
            return Err(CouncilError::OverBudget {
                estimated_usd,
                cap_usd,
            });
        }
    }

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

    let mut inflight: FuturesUnordered<_> = live
        .into_iter()
        .enumerate()
        .map(|(i, (p, model))| {
            let cfg = SubAgentConfig {
                name: p.spec.clone(),
                prompt: task.to_string(),
                max_turns: roster.proposer_max_turns,
                max_tokens: DEFAULT_PROPOSER_MAX_TOKENS,
                system_prompt: None,
                provider: Some(p.spec.clone()),
                model: model.clone(),
            };
            let provider = p.provider.clone();
            async move {
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

    // 4. Quorum — at least `min_proposers` usable proposals.
    let usable = proposals.iter().filter(|p| p.is_usable()).count();
    if usable < roster.min_proposers {
        return Err(CouncilError::InsufficientProposals {
            got: usable,
            need: roster.min_proposers,
        });
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
                let agg = LlmSynthesisAggregator::new(provider, model, base.clone());
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
            proposer_deadline_s: 90,
            global_deadline_s: 25,
            max_cost_usd: None,
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
}
