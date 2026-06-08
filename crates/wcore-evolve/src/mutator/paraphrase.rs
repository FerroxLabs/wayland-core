//! Paraphrase mutator. The ONLY mutator that touches an LLM provider.
//!
//! Determinism contract is fixture-replay: real-provider drift is documented
//! as out-of-contract. Real-provider calls live behind `--features network-tests`
//! and `#[ignore]`. Production `LlmParaphraseProvider` wiring lands in Task 4.

use std::sync::Arc;

use super::{Mutation, MutationError, MutationKind, MutationSeed, Mutator};

/// Provider trait for the Paraphrase mutator. Returns a paraphrased copy of
/// `body` or an error string if unreachable. Synchronous blocking signature
/// because the per-child timeout in `Generation::run` (W10B Task 2) wraps
/// the entire mutator + score call site; the provider itself does not need
/// to know about async cancellation.
pub trait ParaphraseProvider: Send + Sync {
    fn paraphrase_blocking(&self, body: &str, seed_token: &str) -> Result<String, String>;
}

/// Passthrough provider — returns the body unchanged.
///
/// **Default for tests; real impl `LlmParaphraseProvider` ships in Wave PA /
/// W10B.1** (see `llm_paraphrase_provider.rs`). Production callers wire the
/// real adapter through `bin/wcore-evolve.rs`; this passthrough remains the
/// safe default for fixture-replay determinism tests and the offline CLI
/// smoke run.
///
/// Determinism contract: byte-equal output for identical input. Trivially
/// satisfied because the function is the identity.
pub struct PassthroughParaphraseProvider;

impl ParaphraseProvider for PassthroughParaphraseProvider {
    fn paraphrase_blocking(&self, body: &str, _seed_token: &str) -> Result<String, String> {
        Ok(body.to_string())
    }
}

/// LLM-driven paraphrase. Holds a provider trait object; on provider error,
/// returns `MutationError::LlmUnavailable` so the generation loop can decide
/// whether to skip-score the child (default: skip).
pub struct Paraphrase {
    pub provider: Arc<dyn ParaphraseProvider>,
    /// Pinned to 0.0 in production for best-effort determinism. The fixture
    /// provider used in tests ignores this entirely.
    pub temperature: f32,
}

impl Mutator for Paraphrase {
    fn mutate(&self, parent_body: &str, seed: MutationSeed) -> Result<Mutation, MutationError> {
        let seed_token = format!(
            "{}/{}/{}",
            seed.parent_hash, seed.generation, seed.child_index
        );
        let body = self
            .provider
            .paraphrase_blocking(parent_body, &seed_token)
            .map_err(MutationError::LlmUnavailable)?;
        Ok(Mutation {
            body,
            kind: MutationKind::Paraphrase,
        })
    }
}
