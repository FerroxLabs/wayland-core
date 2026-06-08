//! `wcore-user-model` — backend abstraction for the user-model layer.
//!
//! v0.7.0 2.B.1: ships the `UserModelBackend` trait + the
//! `LocalBackend` reference impl (in-memory with optional JSON
//! persistence). 2.B.2 wires Honcho behind the same trait; 2.B.3
//! adds the `PreferenceLearner` + `ExpertiseEstimator` that write
//! signals through this trait; 2.B.4 plugs the trait into the
//! turn-loop middleware.
//!
//! Design: backend-as-port, not as-implementation. The engine
//! depends only on the trait — production deploys may swap
//! `LocalBackend` for `HonchoBackend` via config.

pub mod brief;
pub mod error;
pub mod expertise;
pub mod local;
pub mod observation;
pub mod preference_learner;
pub mod preferences;

pub use brief::{DialecticInference, UserBrief, UserStyle};
pub use error::UserModelError;
pub use expertise::ExpertiseEstimator;
pub use local::LocalBackend;
pub use observation::{Observation, Outcome, ToolHint};
pub use preference_learner::{
    DomainRecommendation, LearnerState, OutcomeCounts, PreferenceLearner,
};
pub use preferences::{ExpertiseLevel, Preferences};

use async_trait::async_trait;

/// Backend-agnostic interface to the user model. The trait is async
/// so backends that span the network (Honcho) plug in without
/// contaminating the synchronous engine path.
#[async_trait]
pub trait UserModelBackend: Send + Sync {
    /// Fetch the current rolling brief for `user_id`. Returns a
    /// default empty `UserBrief` when the user is new.
    async fn brief(&self, user_id: &str) -> Result<UserBrief, UserModelError>;

    /// Fetch current learned preferences for `user_id`.
    async fn preferences(&self, user_id: &str) -> Result<Preferences, UserModelError>;

    /// Record one observation. Backends fold it into the running
    /// estimate however they choose (Bayesian, EMA, counter — opaque
    /// to the caller).
    async fn observe(&self, user_id: &str, obs: Observation) -> Result<(), UserModelError>;

    /// Backend tag, e.g. `"local"`, `"honcho"`. Surfaces in
    /// observability spans so a trace shows which backend served a
    /// given turn.
    fn backend_tag(&self) -> &str;
}
