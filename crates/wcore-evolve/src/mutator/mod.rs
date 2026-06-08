//! W10B: deterministic-seeded skill body mutations.
//!
//! Each `Mutator` consumes a parent body + a `MutationSeed` and produces a
//! `Mutation { body, kind }`. The seed is the **only** source of randomness;
//! the same `(parent_hash, generation, child_index)` always produces the same
//! mutated body. This makes evolutionary runs reproducible and debuggable —
//! a graveyard entry can always be regenerated from its lineage triple.
//!
//! LLM-in-the-loop is **only** for `paraphrase`. Every other strategy is pure
//! deterministic Rust. The scoring path NEVER touches an LLM — W10A's harness
//! is the trust boundary.

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MutationKind {
    Paraphrase,
    Reorder,
    SwapSynonym,
    Precondition,
}

#[derive(Debug, Clone)]
pub struct MutationSeed {
    pub parent_hash: String,
    pub generation: u32,
    pub child_index: u32,
}

impl MutationSeed {
    pub fn new(parent_hash: impl Into<String>, generation: u32, child_index: u32) -> Self {
        Self {
            parent_hash: parent_hash.into(),
            generation,
            child_index,
        }
    }

    /// Derive a 32-byte ChaCha20 seed from the lineage triple via blake3.
    /// Same triple → same 32-byte digest → same `ChaCha20Rng` → same output.
    pub(crate) fn rng(&self) -> ChaCha20Rng {
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.parent_hash.as_bytes());
        hasher.update(&self.generation.to_le_bytes());
        hasher.update(&self.child_index.to_le_bytes());
        let digest = hasher.finalize();
        ChaCha20Rng::from_seed(*digest.as_bytes())
    }
}

#[derive(Debug, Clone)]
pub struct Mutation {
    pub body: String,
    pub kind: MutationKind,
}

#[derive(Debug, thiserror::Error)]
pub enum MutationError {
    #[error("parent body has no `## Steps` section to reorder")]
    NoStepsSection,
    #[error("parent body has no `## Preconditions` section")]
    NoPreconditionsSection,
    #[error("paraphrase provider unavailable: {0}")]
    LlmUnavailable(String),
    #[error("synonym table exhausted for parent body")]
    NoSynonymCandidate,
}

pub trait Mutator: Send + Sync {
    fn mutate(&self, parent_body: &str, seed: MutationSeed) -> Result<Mutation, MutationError>;
}

pub mod llm_paraphrase_provider;
pub mod paraphrase;
pub mod precondition;
pub mod reorder;
pub mod swap_synonym;

pub use llm_paraphrase_provider::{
    AsyncParaphrase, DEFAULT_MAX_TOKENS, DEFAULT_PARAPHRASE_SYSTEM_PROMPT, DEFAULT_REQUEST_TIMEOUT,
    LlmParaphraseError, LlmParaphraseProvider,
};
pub use paraphrase::{Paraphrase, ParaphraseProvider, PassthroughParaphraseProvider};
pub use precondition::Precondition;
pub use reorder::Reorder;
pub use swap_synonym::SwapSynonym;
