//! The council aggregator — fuses fenced proposals into one answer.
//!
//! [`LlmSynthesisAggregator`] runs the synthesis as a **read-only** sub-agent
//! on a pinned provider: it builds the fenced prompt (see [`super::proposal`]),
//! spawns via `AgentSpawner::spawn_one` (whose default tool registry is
//! read-only — no Bash/Write/Edit), and returns the synthesized text. If the
//! aggregator sub-agent itself errors, it falls back to the first usable
//! proposal so a transient aggregator failure never sinks the whole council.

use std::sync::Arc;

use async_trait::async_trait;

use wcore_config::config::Config;
use wcore_providers::LlmProvider;
use wcore_types::message::TokenUsage;

use super::proposal::{AggregateResult, Proposal, build_synthesis_prompt};
use crate::spawner::{AgentSpawner, SubAgentConfig};

/// Fuses council proposals into a single answer.
#[async_trait]
pub trait Aggregator: Send + Sync {
    /// Synthesize one answer to `task` from `proposals`. Implementations MUST
    /// treat proposal text as untrusted data (the fencing in
    /// [`build_synthesis_prompt`] enforces this structurally).
    async fn aggregate(&self, task: &str, proposals: &[Proposal]) -> AggregateResult;
}

/// Per-aggregate turn budget. Synthesis is single-shot; a tiny budget keeps a
/// stuck aggregator from burning the council's cost ceiling. `pub` so the
/// pre-flight cost estimate prices the judge against the SAME ceiling the
/// executor enforces (they must never drift, or the cap undercounts).
pub const AGGREGATOR_MAX_TURNS: usize = 2;
/// Per-aggregate output-token budget. `pub` for the same shared-ceiling reason.
pub const AGGREGATOR_MAX_TOKENS: u32 = 4096;

/// An aggregator that asks a pinned LLM to synthesize the proposals.
pub struct LlmSynthesisAggregator {
    /// The provider the synthesis runs on (already keyed/resolved).
    provider: Arc<dyn LlmProvider>,
    /// Optional model override for the synthesis.
    model: Option<String>,
    /// Base config the synthesis sub-agent inherits (policy surface, etc.).
    base: Config,
    /// Crucible #3: sampling temperature for the synthesis sub-agent
    /// (convergence — runs cooler than the proposers).
    temperature: f32,
}

impl LlmSynthesisAggregator {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        model: Option<String>,
        base: Config,
        temperature: f32,
    ) -> Self {
        Self {
            provider,
            model,
            base,
            temperature,
        }
    }

    /// The first usable proposal's text — the fallback when the aggregator
    /// sub-agent errors (or there is nothing to synthesize).
    fn first_usable_text(proposals: &[Proposal]) -> Option<(String, String)> {
        proposals
            .iter()
            .find(|p| p.is_usable())
            .map(|p| (p.text.clone(), p.provider.clone()))
    }
}

#[async_trait]
impl Aggregator for LlmSynthesisAggregator {
    async fn aggregate(&self, task: &str, proposals: &[Proposal]) -> AggregateResult {
        let chosen_from: Vec<String> = proposals
            .iter()
            .filter(|p| p.is_usable())
            .map(|p| p.provider.clone())
            .collect();

        // Nothing usable to synthesize — surface empty rather than spawn.
        if chosen_from.is_empty() {
            return AggregateResult {
                final_text: String::new(),
                chosen_from,
                rationale: None,
                usage: TokenUsage::default(),
            };
        }

        let prompt = build_synthesis_prompt(task, proposals);

        // Read-only by construction: spawn_one builds the default (read-only)
        // tool registry, so even a successful injection in a proposal cannot
        // reach a side-effecting tool. The provider is the (already-resolved)
        // aggregator provider; `model` is applied via child_config (T2).
        let spawner = AgentSpawner::new(self.provider.clone(), self.base.clone());
        let result = spawner
            .spawn_one(SubAgentConfig {
                name: "__council_aggregator__".to_string(),
                prompt,
                max_turns: AGGREGATOR_MAX_TURNS,
                max_tokens: AGGREGATOR_MAX_TOKENS,
                system_prompt: Some(super::run::COUNCIL_AGGREGATOR_SYSTEM_PROMPT.to_string()),
                provider: None,
                model: self.model.clone(),
                // Crucible #3: aggregator runs cooler for a stable synthesis.
                temperature: Some(self.temperature),
            })
            .await;

        // The aggregator's synthesis sub-agent burned tokens whether it
        // succeeded or errored — capture them for spend accounting.
        let usage = result.usage;

        if result.is_error || result.text.trim().is_empty() {
            // Aggregator failed — fall back to the first usable proposal so a
            // transient synthesis error never sinks the whole council.
            if let Some((text, provider)) = Self::first_usable_text(proposals) {
                return AggregateResult {
                    final_text: text,
                    chosen_from: vec![provider],
                    rationale: Some(
                        "aggregator failed; returned first usable proposal".to_string(),
                    ),
                    usage,
                };
            }
        }

        AggregateResult {
            final_text: result.text,
            chosen_from,
            rationale: None,
            usage,
        }
    }
}

#[cfg(test)]
mod tests {
    // Engine-running aggregator tests (real spawn) live in
    // `tests/crucible_council.rs`, where the proven `common::test_config()` and
    // a capturing provider are available. These inline tests cover the logic
    // that does NOT spawn the engine.
    use tokio::sync::mpsc;
    use wcore_providers::ProviderError;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::TokenUsage;

    use super::*;
    use crate::orchestration::council::proposal::Proposal;

    fn prop(provider: &str, text: &str, is_error: bool) -> Proposal {
        Proposal {
            provider: provider.to_string(),
            model: None,
            text: text.to_string(),
            is_error,
            usage: TokenUsage::default(),
            latency_ms: 0,
        }
    }

    /// Never streams — these tests never reach a spawn.
    struct NeverProvider;

    #[async_trait]
    impl LlmProvider for NeverProvider {
        async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Err(ProviderError::Connection("never".into()))
        }
    }

    #[tokio::test]
    async fn empty_usable_set_returns_empty_without_spawning() {
        // All errored → nothing usable → early return BEFORE any spawn, so the
        // (never-streaming) provider is never invoked.
        let agg =
            LlmSynthesisAggregator::new(Arc::new(NeverProvider), None, Config::default(), 0.4);
        let res = agg.aggregate("task", &[prop("openai", "x", true)]).await;
        assert!(res.final_text.is_empty());
        assert!(res.chosen_from.is_empty());
    }

    #[test]
    fn first_usable_text_picks_first_non_error_nonblank() {
        let proposals = vec![
            prop("openai", "  ", false),  // blank
            prop("anthropic", "x", true), // error
            prop("google", "the answer", false),
        ];
        let (text, provider) = LlmSynthesisAggregator::first_usable_text(&proposals).unwrap();
        assert_eq!(text, "the answer");
        assert_eq!(provider, "google");
    }
}
