//! `wcore-dispatch` — internal decision router for Wayland-Core.
//!
//! This crate selects *templates*, *agents*, and *skills* given task
//! context. It is NOT a model router (that lives in Flux as a separate
//! product). The router learns from observed outcomes via a Thompson-
//! sampling Beta scorer; every choice the router makes can be fed back
//! with [`TaskOutcome`] to update the posterior for future selections.
//!
//! # Layering
//!
//! ```text
//!   wcore-dispatch  (this crate — generic trait + scorer)
//!         ▲
//!         ├── wcore-dispatch::template_router  (4.A.2)
//!         ├── wcore-dispatch::agent_router     (4.A.3, uses wcore-agents-pack)
//!         └── wcore-skills::router             (4.A.4, extends prioritizer)
//! ```
//!
//! # Quick start
//!
//! ```ignore
//! use wcore_dispatch::{BetaScorer, DecisionRouter, RouterError, Scorer, TaskOutcome};
//!
//! struct MyRouter {
//!     scorer: BetaScorer<String>,
//! }
//!
//! impl DecisionRouter<String, &str> for MyRouter {
//!     fn choose(&mut self, _input: &str) -> Result<String, RouterError> {
//!         // Pick the arm whose Beta(α+1, β+1) posterior draws highest this
//!         // turn. Cold-start arms (no observations) share a flat Uniform.
//!         self.scorer.thompson_pick(&[
//!             "Direct".to_string(),
//!             "Consensus".to_string(),
//!             "SelfCritique".to_string(),
//!         ])
//!     }
//!
//!     fn observe(&mut self, choice: &String, outcome: TaskOutcome) {
//!         self.scorer.record(choice, outcome);
//!     }
//! }
//! ```

pub mod agent_router;
pub mod scorer;
pub mod template_router;

pub use agent_router::AgentRouter;
pub use scorer::{BetaScorer, Scorer, Stats};
pub use template_router::{Template, TemplateParseError, TemplateRouter};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Outcome observed after a routing decision was acted on. Feeds the
/// scorer's posterior update. The router itself doesn't decide what
/// counts as "success" — callers map domain signals to one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskOutcome {
    /// Task completed and the choice contributed positively.
    Success,
    /// Task either failed or the choice was unhelpful / regressed.
    Failure,
    /// Task didn't run to a verdict (cancelled, interrupted, ambiguous).
    /// Scorer ignores these — neither α nor β increments.
    Neutral,
}

/// Errors a router can surface. Concrete routers may wrap these with
/// crate-local context if they need richer reporting.
#[derive(Debug, Error)]
pub enum RouterError {
    #[error("router has no candidates available for input")]
    NoCandidates,
    #[error("router declined to pick (e.g. all candidates disqualified): {reason}")]
    Declined { reason: String },
    #[error("router internal error: {0}")]
    Internal(String),
}

/// Generic decision router. Implementors choose a `TKey` (typically a
/// `String` or a small enum) given some `TInput` and learn from
/// observed `TaskOutcome`s tied back to the chosen key.
///
/// Routers are usually NOT `Send + Sync` by themselves — the embedding
/// orchestrator wraps them in a `Mutex` when crossing async boundaries.
pub trait DecisionRouter<TKey, TInput> {
    /// Pick the best candidate for this input. May fail with
    /// `RouterError::NoCandidates` if no arms are configured, or
    /// `RouterError::Declined` if every candidate was filtered out.
    fn choose(&mut self, input: TInput) -> Result<TKey, RouterError>;

    /// Update the scorer with the observed outcome of a prior choice.
    /// Implementations should be idempotent for `TaskOutcome::Neutral`.
    fn observe(&mut self, choice: &TKey, outcome: TaskOutcome);
}
