//! M5.2 — typed errors for trace load / replay / diff.
//!
//! `VersionSkew` is recoverable (caller can opt in via
//! `--force-version-skew` on the CLI). `Divergent` is informational —
//! callers should surface it as a user-visible diagnostic rather than
//! treating it as a fatal error.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("trace io: {0}")]
    Io(#[from] std::io::Error),
    #[error("trace decode: {0}")]
    Decode(#[from] serde_json::Error),
    #[error(
        "version skew: trace {trace}, runtime {runtime} (pass --force-version-skew to override)"
    )]
    VersionSkew { trace: String, runtime: String },
    #[error("divergent replay at event {event_index}: {reason}")]
    Divergent { event_index: usize, reason: String },
}

pub type Result<T> = std::result::Result<T, ReplayError>;
