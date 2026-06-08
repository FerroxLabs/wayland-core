//! # wcore-evolve
//!
//! W10B: F12 GEPA evolution loop. Reads candidates seeded from W9 F10 drafts,
//! mutates them deterministically, scores them through W10A's `wcore-eval`
//! harness, and promotes winners back through the W9 F11 curator.
//!
//! See `docs/superpowers/plans/2026-05-15-wcore-W10B-gepa.md` for the full plan.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

pub mod curator_handoff;
pub mod error;
pub mod evolve;
pub mod generation;
pub mod mutator;
pub mod prompt_store;
pub mod schema_reward;

pub use error::EvolveError;
pub use evolve::{
    EvolveOutcome, EvolveParams, NullTraceSink, PlateauDetector, PlateauError, TerminationReason,
    TraceSink, evolve,
};
pub use prompt_store::{EvolvedPrompt, PromptStore};
pub use schema_reward::{SchemaRewardScore, ToolCallObservation, ToolCallSchemaReward};
