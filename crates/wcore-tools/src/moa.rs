//! T2-C2: Mixture-of-Agents tool.
//!
//! Port of Wang 2024 ("Mixture-of-Agents Enhances Large Language Model
//! Capabilities", arXiv:2406.04692) from the prior Wayland Python engine.
//!
//! The MoA pattern fans a user prompt across N *proposer* providers in
//! parallel, concatenates their answers, then hands the bundle to an
//! *aggregator* provider that synthesizes a single final answer.
//!
//! This module is intentionally **provider-agnostic** — instead of
//! wiring it directly to any concrete `LlmProvider`, it takes a
//! [`ProposerCaller`] trait object so callers (and tests) can stub
//! the LLM hop. The agent layer will compose this with the real
//! provider registry; tests can fan over a `MockCaller`.
//!
//! See `crates/wcore-tools/src/moa.rs` tests below for usage.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// Aggregator system-prompt prefix from the MoA paper. Successful
/// proposer outputs are appended (newline-joined, 1-indexed) before
/// the aggregator is called.
pub const AGGREGATOR_SYSTEM_PROMPT: &str = "You have been provided with a set of responses from various open-source models to the latest user query. Your task is to synthesize these responses into a single, high-quality response. It is crucial to critically evaluate the information provided in these responses, recognizing that some of it may be biased or incorrect. Your response should not simply replicate the given answers but should offer a refined, accurate, and comprehensive reply to the instruction. Ensure your response is well-structured, coherent, and adheres to the highest standards of accuracy and reliability.\n\nResponses from models:";

/// One proposer (or the aggregator) — a provider+model+sampling triple.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProposerSpec {
    /// Provider id (e.g. `"anthropic"`, `"openai"`, `"openrouter"`).
    pub provider_id: String,
    /// Model id (e.g. `"claude-opus-4-7"`).
    pub model: String,
    /// Sampling temperature.
    pub temperature: f32,
    /// Optional system prompt for this proposer. The aggregator's
    /// effective system prompt is built by `execute` from the
    /// proposer outputs and [`AGGREGATOR_SYSTEM_PROMPT`]; if this
    /// field is set on the aggregator spec it is prepended to that
    /// synthesized prompt.
    pub system_prompt: Option<String>,
}

/// Input to the MoA tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoaInput {
    /// The end-user message to route through MoA.
    pub user_message: String,
    /// Per-proposer max output tokens.
    pub max_proposer_tokens: usize,
    /// Aggregator max output tokens.
    pub max_aggregator_tokens: usize,
}

/// A single proposer's output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProposerOutput {
    pub provider_id: String,
    pub model: String,
    pub content: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Final output from a MoA run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoaOutput {
    /// Synthesized aggregator answer.
    pub aggregated: String,
    /// Per-proposer outputs (only successful proposers; ordering
    /// matches the order in `MoaTool::proposers`).
    pub proposer_outputs: Vec<ProposerOutput>,
    /// Sum of input tokens across all proposers + aggregator.
    pub total_input_tokens: u64,
    /// Sum of output tokens across all proposers + aggregator.
    pub total_output_tokens: u64,
}

/// MoA failure modes.
#[derive(Debug, Error)]
pub enum MoaError {
    #[error("proposer {provider_id} failed: {source}")]
    ProposerFailed {
        provider_id: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("aggregator failed: {source}")]
    AggregatorFailed {
        /// `Some(provider_id)` when the failure originated from the
        /// aggregator's underlying provider call (re-wrapped from
        /// `ProposerFailed`). `None` when the aggregator-trait layer
        /// surfaced a failure without a provider attribution.
        provider_id: Option<String>,
        #[source]
        source: anyhow::Error,
    },
    #[error("all proposers failed (no successful responses)")]
    AllProposersFailed,
}

/// Abstraction over the LLM call so unit tests can stub it. Concrete
/// implementations in the agent layer dispatch to real providers.
///
/// The same trait is used for proposer and aggregator hops; the
/// `spec` discriminates. The `input.user_message` carries the
/// already-constructed prompt for the aggregator hop.
#[async_trait]
pub trait ProposerCaller: Send + Sync {
    async fn call(&self, spec: &ProposerSpec, input: &MoaInput)
    -> Result<ProposerOutput, MoaError>;
}

/// Mixture-of-Agents tool — fan-out + synthesize.
#[derive(Debug, Clone)]
pub struct MoaTool {
    pub proposers: Vec<ProposerSpec>,
    pub aggregator: ProposerSpec,
    /// Cap on simultaneous proposer calls. `0` is treated as "no
    /// cap" (all proposers fire at once).
    pub max_concurrency: usize,
}

impl MoaTool {
    pub fn new(
        proposers: Vec<ProposerSpec>,
        aggregator: ProposerSpec,
        max_concurrency: usize,
    ) -> Self {
        Self {
            proposers,
            aggregator,
            max_concurrency,
        }
    }

    /// Build the aggregator user-prompt body by enumerating successful
    /// proposer outputs (1-indexed, mirrors the Python source).
    ///
    /// If `aggregator_system_prompt` is `Some`, it is prepended verbatim
    /// (followed by `"\n\n"`) before the synthesized [`AGGREGATOR_SYSTEM_PROMPT`].
    /// This makes the ProposerSpec docstring honest — previously the
    /// aggregator's `system_prompt` field was never consulted by `execute`.
    fn construct_aggregator_user_prompt(
        user_message: &str,
        proposer_outputs: &[ProposerOutput],
        aggregator_system_prompt: Option<&str>,
    ) -> String {
        let prepend_len = aggregator_system_prompt.map(|s| s.len() + 2).unwrap_or(0);
        let mut parts = String::with_capacity(
            prepend_len + AGGREGATOR_SYSTEM_PROMPT.len() + user_message.len() + 256,
        );
        if let Some(sp) = aggregator_system_prompt {
            parts.push_str(sp);
            parts.push_str("\n\n");
        }
        parts.push_str(AGGREGATOR_SYSTEM_PROMPT);
        parts.push_str("\n\n");
        for (i, out) in proposer_outputs.iter().enumerate() {
            parts.push_str(&format!("{}. {}\n", i + 1, out.content));
        }
        parts.push_str("\nOriginal user query:\n");
        parts.push_str(user_message);
        parts
    }

    /// Run the MoA pipeline.
    pub async fn execute<C: ProposerCaller + 'static>(
        &self,
        caller: &Arc<C>,
        input: &MoaInput,
    ) -> Result<MoaOutput, MoaError> {
        // Empty proposer list — treat as all-failed so callers see a
        // single error variant rather than a degenerate aggregator
        // call on an empty bundle.
        if self.proposers.is_empty() {
            return Err(MoaError::AllProposersFailed);
        }

        // Concurrency cap: `0` => unbounded (use `Semaphore::MAX_PERMITS`).
        let permits = if self.max_concurrency == 0 {
            Semaphore::MAX_PERMITS
        } else {
            self.max_concurrency
        };
        let semaphore = Arc::new(Semaphore::new(permits));

        let mut join_set: JoinSet<(usize, Result<ProposerOutput, MoaError>)> = JoinSet::new();
        for (idx, spec) in self.proposers.iter().enumerate() {
            let spec = spec.clone();
            let input = input.clone();
            let caller = caller.clone();
            let semaphore = semaphore.clone();
            let provider_id = spec.provider_id.clone();
            join_set.spawn(async move {
                // `acquire_owned` ties the permit to the future's
                // lifetime — when the task exits the permit is
                // released, giving the next queued task a slot.
                // If the semaphore is closed, surface as a ProposerFailed
                // rather than panicking the task.
                let _permit = match semaphore.acquire_owned().await {
                    Ok(p) => p,
                    Err(e) => {
                        return (
                            idx,
                            Err(MoaError::ProposerFailed {
                                provider_id: provider_id.clone(),
                                source: anyhow::anyhow!("MoA semaphore closed: {e}"),
                            }),
                        );
                    }
                };
                let result = caller.call(&spec, &input).await;
                (idx, result)
            });
        }

        // Collect outputs preserving proposer-spec order. Failures
        // are dropped (matching the Python behavior of running the
        // aggregator on successful responses only). A panicked task
        // is surfaced as a ProposerFailed rather than re-panicking
        // the caller.
        let mut indexed: Vec<Option<ProposerOutput>> =
            (0..self.proposers.len()).map(|_| None).collect();
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((idx, res)) => {
                    if let Ok(output) = res {
                        indexed[idx] = Some(output);
                    }
                }
                Err(_join_err) => {
                    // Task panicked or was cancelled — drop it like any
                    // other proposer failure; the aggregator will see only
                    // successful outputs.
                }
            }
        }
        let proposer_outputs: Vec<ProposerOutput> = indexed.into_iter().flatten().collect();

        if proposer_outputs.is_empty() {
            return Err(MoaError::AllProposersFailed);
        }

        // Aggregator hop. Build a synthetic MoaInput whose
        // `user_message` is the aggregator prompt body.
        let aggregator_user_prompt = Self::construct_aggregator_user_prompt(
            &input.user_message,
            &proposer_outputs,
            self.aggregator.system_prompt.as_deref(),
        );
        let aggregator_input = MoaInput {
            user_message: aggregator_user_prompt,
            max_proposer_tokens: input.max_proposer_tokens,
            max_aggregator_tokens: input.max_aggregator_tokens,
        };
        let aggregator_result = caller.call(&self.aggregator, &aggregator_input).await;
        let aggregator_output = match aggregator_result {
            Ok(out) => out,
            Err(MoaError::ProposerFailed {
                provider_id,
                source,
            }) => {
                return Err(MoaError::AggregatorFailed {
                    provider_id: Some(provider_id),
                    source,
                });
            }
            Err(other) => return Err(other),
        };

        let total_input_tokens: u64 = proposer_outputs.iter().map(|o| o.input_tokens).sum::<u64>()
            + aggregator_output.input_tokens;
        let total_output_tokens: u64 = proposer_outputs
            .iter()
            .map(|o| o.output_tokens)
            .sum::<u64>()
            + aggregator_output.output_tokens;

        Ok(MoaOutput {
            aggregated: aggregator_output.content,
            proposer_outputs,
            total_input_tokens,
            total_output_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn spec(provider: &str, model: &str) -> ProposerSpec {
        ProposerSpec {
            provider_id: provider.to_string(),
            model: model.to_string(),
            temperature: 0.6,
            system_prompt: None,
        }
    }

    fn input() -> MoaInput {
        MoaInput {
            user_message: "solve P=NP".into(),
            max_proposer_tokens: 256,
            max_aggregator_tokens: 512,
        }
    }

    /// Mock caller — routes per-spec by `(provider_id, model)` key to a
    /// scripted result. Aggregator gets a separate `aggregator` slot.
    /// Tracks in-flight count so concurrency tests can assert caps.
    struct MockCaller {
        proposer_results: Mutex<std::collections::HashMap<String, Result<ProposerOutput, String>>>,
        aggregator_result: Mutex<Option<Result<ProposerOutput, String>>>,
        in_flight: AtomicUsize,
        max_observed_in_flight: AtomicUsize,
        proposer_delay: Duration,
    }

    impl MockCaller {
        fn new() -> Self {
            Self {
                proposer_results: Mutex::new(std::collections::HashMap::new()),
                aggregator_result: Mutex::new(None),
                in_flight: AtomicUsize::new(0),
                max_observed_in_flight: AtomicUsize::new(0),
                proposer_delay: Duration::from_millis(0),
            }
        }

        fn key(provider: &str, model: &str) -> String {
            format!("{provider}::{model}")
        }

        fn set_proposer_ok(
            &self,
            provider: &str,
            model: &str,
            content: &str,
            in_tok: u64,
            out_tok: u64,
        ) {
            self.proposer_results.lock().unwrap().insert(
                Self::key(provider, model),
                Ok(ProposerOutput {
                    provider_id: provider.into(),
                    model: model.into(),
                    content: content.into(),
                    input_tokens: in_tok,
                    output_tokens: out_tok,
                }),
            );
        }

        fn set_proposer_err(&self, provider: &str, model: &str, msg: &str) {
            self.proposer_results
                .lock()
                .unwrap()
                .insert(Self::key(provider, model), Err(msg.to_string()));
        }

        fn set_aggregator_ok(&self, content: &str, in_tok: u64, out_tok: u64) {
            *self.aggregator_result.lock().unwrap() = Some(Ok(ProposerOutput {
                provider_id: "aggregator".into(),
                model: "agg-model".into(),
                content: content.into(),
                input_tokens: in_tok,
                output_tokens: out_tok,
            }));
        }

        fn set_aggregator_err(&self, msg: &str) {
            *self.aggregator_result.lock().unwrap() = Some(Err(msg.to_string()));
        }
    }

    #[async_trait]
    impl ProposerCaller for MockCaller {
        async fn call(
            &self,
            spec: &ProposerSpec,
            _input: &MoaInput,
        ) -> Result<ProposerOutput, MoaError> {
            // Aggregator path: provider_id == "aggregator".
            if spec.provider_id == "aggregator" {
                let slot = self.aggregator_result.lock().unwrap().clone();
                return match slot {
                    Some(Ok(out)) => Ok(out),
                    Some(Err(msg)) => Err(MoaError::AggregatorFailed {
                        provider_id: None,
                        source: anyhow::anyhow!(msg),
                    }),
                    None => Err(MoaError::AggregatorFailed {
                        provider_id: None,
                        source: anyhow::anyhow!("no aggregator result scripted"),
                    }),
                };
            }

            // Track concurrency.
            let before = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            let mut max = self.max_observed_in_flight.load(Ordering::SeqCst);
            while before > max {
                match self.max_observed_in_flight.compare_exchange(
                    max,
                    before,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => break,
                    Err(actual) => max = actual,
                }
            }
            if !self.proposer_delay.is_zero() {
                tokio::time::sleep(self.proposer_delay).await;
            }
            let key = Self::key(&spec.provider_id, &spec.model);
            let result = self.proposer_results.lock().unwrap().get(&key).cloned();
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            match result {
                Some(Ok(out)) => Ok(out),
                Some(Err(msg)) => Err(MoaError::ProposerFailed {
                    provider_id: spec.provider_id.clone(),
                    source: anyhow::anyhow!(msg),
                }),
                None => Err(MoaError::ProposerFailed {
                    provider_id: spec.provider_id.clone(),
                    source: anyhow::anyhow!("no scripted result for {key}"),
                }),
            }
        }
    }

    fn three_proposers() -> Vec<ProposerSpec> {
        vec![
            spec("anthropic", "claude"),
            spec("openai", "gpt"),
            spec("google", "gemini"),
        ]
    }

    fn aggregator_spec() -> ProposerSpec {
        ProposerSpec {
            provider_id: "aggregator".into(),
            model: "agg-model".into(),
            temperature: 0.4,
            system_prompt: None,
        }
    }

    #[tokio::test]
    async fn all_proposers_succeed_aggregator_runs() {
        let mock = Arc::new(MockCaller::new());
        mock.set_proposer_ok("anthropic", "claude", "A1", 10, 20);
        mock.set_proposer_ok("openai", "gpt", "A2", 11, 21);
        mock.set_proposer_ok("google", "gemini", "A3", 12, 22);
        mock.set_aggregator_ok("SYNTHESIS", 100, 200);

        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 4);
        let out = tool.execute(&mock, &input()).await.unwrap();

        assert_eq!(out.aggregated, "SYNTHESIS");
        assert_eq!(out.proposer_outputs.len(), 3);
    }

    #[tokio::test]
    async fn proposer_outputs_collected_in_order() {
        let mock = Arc::new(MockCaller::new());
        mock.set_proposer_ok("anthropic", "claude", "first", 1, 2);
        mock.set_proposer_ok("openai", "gpt", "second", 3, 4);
        mock.set_proposer_ok("google", "gemini", "third", 5, 6);
        mock.set_aggregator_ok("synth", 0, 0);

        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 4);
        let out = tool.execute(&mock, &input()).await.unwrap();

        let contents: Vec<&str> = out
            .proposer_outputs
            .iter()
            .map(|o| o.content.as_str())
            .collect();
        assert_eq!(contents, vec!["first", "second", "third"]);
        let providers: Vec<&str> = out
            .proposer_outputs
            .iter()
            .map(|o| o.provider_id.as_str())
            .collect();
        assert_eq!(providers, vec!["anthropic", "openai", "google"]);
    }

    #[tokio::test]
    async fn one_proposer_fails_others_succeed() {
        let mock = Arc::new(MockCaller::new());
        mock.set_proposer_ok("anthropic", "claude", "ok1", 1, 1);
        mock.set_proposer_err("openai", "gpt", "rate-limited");
        mock.set_proposer_ok("google", "gemini", "ok3", 1, 1);
        mock.set_aggregator_ok("agg", 0, 0);

        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 4);
        let out = tool.execute(&mock, &input()).await.unwrap();

        assert_eq!(out.proposer_outputs.len(), 2);
        let contents: Vec<&str> = out
            .proposer_outputs
            .iter()
            .map(|o| o.content.as_str())
            .collect();
        // Order preserved among survivors.
        assert_eq!(contents, vec!["ok1", "ok3"]);
        assert_eq!(out.aggregated, "agg");
    }

    #[tokio::test]
    async fn all_proposers_fail_returns_all_proposers_failed_error() {
        let mock = Arc::new(MockCaller::new());
        mock.set_proposer_err("anthropic", "claude", "down");
        mock.set_proposer_err("openai", "gpt", "down");
        mock.set_proposer_err("google", "gemini", "down");
        // Aggregator scripted but should never fire.
        mock.set_aggregator_ok("must-not-appear", 0, 0);

        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 4);
        let err = tool.execute(&mock, &input()).await.unwrap_err();
        assert!(matches!(err, MoaError::AllProposersFailed));
    }

    #[tokio::test]
    async fn aggregator_failure_propagates() {
        let mock = Arc::new(MockCaller::new());
        mock.set_proposer_ok("anthropic", "claude", "x", 1, 1);
        mock.set_proposer_ok("openai", "gpt", "y", 1, 1);
        mock.set_proposer_ok("google", "gemini", "z", 1, 1);
        mock.set_aggregator_err("aggregator boom");

        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 4);
        let err = tool.execute(&mock, &input()).await.unwrap_err();
        match err {
            MoaError::AggregatorFailed {
                provider_id,
                source,
            } => {
                // Aggregator-trait direct failure — no provider attribution.
                assert!(provider_id.is_none());
                assert!(source.to_string().contains("aggregator boom"));
            }
            other => panic!("expected AggregatorFailed, got {other:?}"),
        }
    }

    /// When the aggregator's underlying provider call surfaces a
    /// `ProposerFailed` (e.g. the aggregator slot routed through a
    /// real provider that returned a per-provider error), the re-wrap
    /// into `AggregatorFailed` must preserve the `provider_id` so
    /// callers can attribute the fault to a specific upstream.
    #[tokio::test]
    async fn aggregator_failure_preserves_provider_id_on_proposer_rewrap() {
        // Caller that synthesizes a ProposerFailed on the aggregator hop.
        struct AggregatorProposerFailCaller;
        #[async_trait]
        impl ProposerCaller for AggregatorProposerFailCaller {
            async fn call(
                &self,
                spec: &ProposerSpec,
                _input: &MoaInput,
            ) -> Result<ProposerOutput, MoaError> {
                if spec.provider_id == "aggregator" {
                    return Err(MoaError::ProposerFailed {
                        provider_id: "anthropic-fallback".to_string(),
                        source: anyhow::anyhow!("upstream 429"),
                    });
                }
                Ok(ProposerOutput {
                    provider_id: spec.provider_id.clone(),
                    model: spec.model.clone(),
                    content: "ok".into(),
                    input_tokens: 1,
                    output_tokens: 1,
                })
            }
        }

        let caller = Arc::new(AggregatorProposerFailCaller);
        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 4);
        let err = tool.execute(&caller, &input()).await.unwrap_err();
        match err {
            MoaError::AggregatorFailed {
                provider_id,
                source,
            } => {
                assert_eq!(provider_id.as_deref(), Some("anthropic-fallback"));
                assert!(source.to_string().contains("429"));
            }
            other => panic!("expected AggregatorFailed, got {other:?}"),
        }
    }

    /// Regression test for the `Err(other) => return Err(other)` arm in
    /// the aggregator-hop match (around line 274). If the aggregator's
    /// underlying caller returns `MoaError::AllProposersFailed` (rather
    /// than `ProposerFailed`), it must propagate unchanged — NOT get
    /// silently rewrapped into `AggregatorFailed`. This preserves
    /// diagnostic clarity for the rare case where the aggregator's
    /// own provider has no proposer-attribution data.
    #[tokio::test]
    async fn aggregator_returning_all_proposers_failed_propagates_unchanged() {
        struct AggregatorAllProposersFailedCaller;
        #[async_trait]
        impl ProposerCaller for AggregatorAllProposersFailedCaller {
            async fn call(
                &self,
                spec: &ProposerSpec,
                _input: &MoaInput,
            ) -> Result<ProposerOutput, MoaError> {
                if spec.provider_id == "aggregator" {
                    return Err(MoaError::AllProposersFailed);
                }
                Ok(ProposerOutput {
                    provider_id: spec.provider_id.clone(),
                    model: spec.model.clone(),
                    content: "ok".into(),
                    input_tokens: 1,
                    output_tokens: 1,
                })
            }
        }

        let caller = Arc::new(AggregatorAllProposersFailedCaller);
        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 4);
        let err = tool.execute(&caller, &input()).await.unwrap_err();
        assert!(
            matches!(err, MoaError::AllProposersFailed),
            "AllProposersFailed from the aggregator hop must propagate \
             unchanged (not be rewrapped as AggregatorFailed), got {err:?}"
        );
    }

    #[tokio::test]
    async fn total_token_counts_sum_correctly() {
        let mock = Arc::new(MockCaller::new());
        mock.set_proposer_ok("anthropic", "claude", "x", 10, 20);
        mock.set_proposer_ok("openai", "gpt", "y", 30, 40);
        mock.set_proposer_ok("google", "gemini", "z", 50, 60);
        mock.set_aggregator_ok("agg", 7, 9);

        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 4);
        let out = tool.execute(&mock, &input()).await.unwrap();
        assert_eq!(out.total_input_tokens, 10 + 30 + 50 + 7);
        assert_eq!(out.total_output_tokens, 20 + 40 + 60 + 9);
    }

    #[tokio::test]
    async fn concurrency_cap_respected() {
        let mut mock = MockCaller::new();
        mock.proposer_delay = Duration::from_millis(40);
        let mock = Arc::new(mock);
        mock.set_proposer_ok("anthropic", "claude", "a", 1, 1);
        mock.set_proposer_ok("openai", "gpt", "b", 1, 1);
        mock.set_proposer_ok("google", "gemini", "c", 1, 1);
        mock.set_aggregator_ok("agg", 0, 0);

        let tool = MoaTool::new(three_proposers(), aggregator_spec(), 1);
        let _ = tool.execute(&mock, &input()).await.unwrap();
        assert_eq!(
            mock.max_observed_in_flight.load(Ordering::SeqCst),
            1,
            "max_concurrency=1 should serialize proposer calls"
        );
    }

    #[tokio::test]
    async fn empty_proposer_list_returns_all_proposers_failed() {
        let mock = Arc::new(MockCaller::new());
        mock.set_aggregator_ok("never", 0, 0);
        let tool = MoaTool::new(vec![], aggregator_spec(), 4);
        let err = tool.execute(&mock, &input()).await.unwrap_err();
        assert!(matches!(err, MoaError::AllProposersFailed));
    }

    #[test]
    fn aggregator_system_prompt_prepended_when_some() {
        let outs = vec![ProposerOutput {
            provider_id: "p".into(),
            model: "m".into(),
            content: "Hello".into(),
            input_tokens: 0,
            output_tokens: 0,
        }];
        let body = MoaTool::construct_aggregator_user_prompt("Q", &outs, Some("CUSTOM-AGG-PROMPT"));
        assert!(
            body.starts_with("CUSTOM-AGG-PROMPT\n\n"),
            "body must begin with the aggregator system_prompt followed by \\n\\n, got: {body:?}"
        );
        // The synthesized AGGREGATOR_SYSTEM_PROMPT still follows.
        assert!(body.contains(AGGREGATOR_SYSTEM_PROMPT));
    }

    #[test]
    fn aggregator_system_prompt_omitted_when_none() {
        let outs = vec![ProposerOutput {
            provider_id: "p".into(),
            model: "m".into(),
            content: "Hello".into(),
            input_tokens: 0,
            output_tokens: 0,
        }];
        let body = MoaTool::construct_aggregator_user_prompt("Q", &outs, None);
        // No custom prepend — body opens with the synthesized prompt.
        assert!(
            body.starts_with(AGGREGATOR_SYSTEM_PROMPT),
            "with no aggregator system_prompt, body must start with AGGREGATOR_SYSTEM_PROMPT"
        );
    }
}
