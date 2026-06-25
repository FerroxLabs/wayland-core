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

use std::time::Instant;

use futures::future::join_all;
use wcore_config::config::Config;

use super::aggregator::{Aggregator, LlmSynthesisAggregator};
use super::proposal::Proposal;
use super::roster::Roster;
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
}

/// Why a council run could not produce a result.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CouncilError {
    /// The spawner has no provider resolver, so no proposer can be keyed.
    #[error("no provider resolver attached to the spawner; cannot run a council")]
    NoResolver,
    /// Fewer usable (non-error) proposals than `min_proposers` required.
    #[error("insufficient council proposals: {got} usable < {need} required")]
    InsufficientProposals { got: usize, need: usize },
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
    //    members / unknown ids) BEFORE spawning. Recorded with their reason.
    let mut live = Vec::new();
    let mut skipped = Vec::new();
    for p in &roster.proposers {
        match resolver.resolve_provider(&p.spec) {
            Ok(_) => live.push(p),
            Err(e) => skipped.push(SkippedProposer {
                spec: p.spec.clone(),
                reason: e.to_string(),
            }),
        }
    }

    // 2. Spawn the survivors concurrently on their pinned providers, timing
    //    each. provider = the full spec so the resolver keys provider+model;
    //    model mirrors it so child_config sets the request model consistently.
    let spawns = live.into_iter().map(|p| {
        let cfg = SubAgentConfig {
            name: p.spec.clone(),
            prompt: task.to_string(),
            max_turns: roster.proposer_max_turns,
            max_tokens: DEFAULT_PROPOSER_MAX_TOKENS,
            system_prompt: None,
            provider: Some(p.spec.clone()),
            model: p.model.clone(),
        };
        async move {
            let start = Instant::now();
            let result = spawner.spawn_one(cfg).await;
            (p.provider.clone(), p.model.clone(), result, start.elapsed())
        }
    });
    let results = join_all(spawns).await;

    // 3. Build proposals with full provenance.
    let proposals: Vec<Proposal> = results
        .into_iter()
        .map(|(provider, model, result, elapsed)| Proposal {
            provider,
            model,
            text: result.text,
            is_error: result.is_error,
            usage: result.usage,
            latency_ms: elapsed.as_millis() as u64,
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
    let aggregate = match &roster.aggregator {
        Some(spec) => match resolver.resolve_provider(spec) {
            Ok((provider, model)) => {
                let agg = LlmSynthesisAggregator::new(provider, model, base.clone());
                Some(agg.aggregate(task, &proposals).await)
            }
            Err(_) => None,
        },
        None => None,
    };

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
