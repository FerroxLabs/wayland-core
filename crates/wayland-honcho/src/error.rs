//! Wave B4 — public error type for `wayland-honcho`.
//!
//! Follows the engine-wide convention: `thiserror` for the public API,
//! so callers can match on variants rather than parse strings.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HonchoError {
    /// Honcho HTTP API returned a non-success status.
    #[error("honcho api: {0}")]
    Api(String),
    /// Underlying transport (DNS, TLS, socket, body decode).
    #[error("honcho transport: {0}")]
    Transport(#[from] reqwest::Error),
    /// Egress chokepoint error (B1): policy denial or transport failure on the
    /// outbound request path.
    #[error("honcho egress: {0}")]
    Egress(#[from] wcore_egress::EgressError),
    /// `HONCHO_API_KEY` missing for live mode (no silent mock fallback per
    /// [[feedback-no-stubs]] — surface the gap honestly).
    #[error("missing HONCHO_API_KEY for live mode")]
    MissingApiKey,
}

pub type Result<T> = std::result::Result<T, HonchoError>;
