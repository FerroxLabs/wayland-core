//! Eval harness for wcore skill candidates.
//!
//! This crate is the **W10A spike** that gates F12 GEPA (W10B). It
//! deterministically classifies candidate skills (good vs bad) and
//! reports precision/recall against a 60-case reference corpus. The
//! harness does NOT mutate skills, does NOT call any LLM in the
//! scoring hot path, and does NOT integrate with `wcore-agent` — the
//! loop is isolated by design so it can be iterated on without
//! touching the agent.
//!
//! See `crates/wcore-eval/README.md` for corpus provenance and the
//! scoring-weights rationale. See
//! `docs/superpowers/specs/2026-05-14-wcore-super-agent-design.md`
//! §5.3 for the design contract this crate satisfies.
//!
//! ## LOCKED PUBLIC SURFACE
//!
//! This crate's surface is frozen as of rev-2 of the W10A plan; W10B
//! depends on it verbatim. See the end of the plan document for the
//! locked surface.

pub mod bench;
pub mod corpus;
pub mod error;
pub mod harness;
pub mod report;
pub mod scorer;

pub use bench::{
    BenchCase, BenchCaseFrontmatter, BenchCategory, BenchCorpus, BenchMatchStrategy, BenchOutcome,
    BenchRunner, BenchScorer, CannedBenchRunner,
};
pub use corpus::{Candidate, Corpus, ExpectedOutcome, ReferenceCase, Verdict, Winner};
pub use error::EvalError;
pub use harness::Harness;
pub use report::{CaseResult, EvalReport};
pub use scorer::{
    DefaultScorer, DefaultScorerConstants, LOCKED, ScoreDimensions, ScoreOutcome, Scorer,
};
