//! Wave B2 — `DenyReason` enum + `PolicyResult` alias.
//!
//! `thiserror`-backed because callers (notably `wcore-agent`) match on
//! `DenyReason` to map to user-visible messages and to decide retry policy.

use thiserror::Error;

/// Reason a `PolicyEngine::check` or `BearerToken::verify` rejected the call.
///
/// `Clone + Eq` so tests can `assert_eq!` and callers can stash the reason in
/// trace events without re-creating it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DenyReason {
    #[error("no matching grant for actor+resource+action")]
    NoMatchingGrant,
    #[error("path not in allowlist for granted resource")]
    PathNotInAllowlist,
    #[error("token expired")]
    TokenExpired,
    #[error("token signature invalid")]
    TokenInvalid,
    #[error("unknown actor")]
    UnknownActor,
    /// Token was previously revoked by a `RevocationStore` (M5.9).
    #[error("token revoked")]
    TokenRevoked,
    /// Backing storage for the revocation store failed (sqlite, lock, I/O).
    /// String payload kept so callers can log the underlying cause without
    /// leaking the original error type into the public API.
    #[error("revocation store error: {0}")]
    Storage(String),
}

/// Result alias used throughout `wcore-permissions`.
pub type PolicyResult<T> = std::result::Result<T, DenyReason>;
