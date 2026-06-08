//! Public error surface for wcore-evolve.

use crate::evolve::graveyard::GraveyardError;
use crate::mutator::MutationError;

#[derive(Debug, thiserror::Error)]
pub enum EvolveError {
    #[error("mutation failed: {0}")]
    Mutation(#[from] MutationError),

    #[error("graveyard io error: {0}")]
    Graveyard(#[from] GraveyardError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("curator rejected winner: {0}")]
    CuratorRejected(String),

    #[error("budget exhausted before any candidate scored")]
    BudgetExhaustedEmpty,

    #[error("provider unavailable for paraphrase: {0}")]
    LlmUnavailable(String),

    #[error("child {child_index} timed out after {timeout:?}")]
    ChildTimedOut {
        child_index: u32,
        timeout: std::time::Duration,
    },

    #[error("prompt store: {0}")]
    PromptStore(String),
}
