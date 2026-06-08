//! `wcore-permissions` — Wave B2 multi-actor ACL + bearer-token authn/authz
//! for wayland-core sessions.
//!
//! v0.3 scope:
//! - Explicit ACL over `(Actor, Resource, Action)` tuples via [`PolicyEngine`].
//! - SHA-256-signed bearer tokens with TTL via [`BearerToken`].
//! - `Actor::System` is the single hard-coded bypass (engine-internal callers).
//!
//! Out-of-scope (post-v0.6 waves):
//! - OAuth / OIDC identity providers.
//! - Role hierarchies, role inheritance, ABAC.
//! - True HMAC / asymmetric (Ed25519) signing.
//!
//! Layer: this crate is mid-layer per `AGENTS.md`. Allowed deps:
//! `wcore-types`, `wcore-config`, third-party. FORBIDDEN: `wcore-agent`,
//! `wcore-cli`. `wcore-agent` depends on us, not the other way around.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod actor;
pub mod error;
pub mod learning;
pub mod policy;
pub mod revocation;
pub mod token;

pub use actor::CallActor;
pub use error::{DenyReason, PolicyResult};
pub use learning::{EvalResult, LearnedDecision, LearnedPolicy, LearningError};
pub use policy::{
    Action, Actor, GrantAuditEvent, GrantAuditSink, Permission, PolicyEngine, Resource,
};
pub use revocation::RevocationStore;
#[cfg(feature = "sqlite-revocation")]
pub use revocation::SqliteRevocationStore;
pub use token::BearerToken;
