//! `ChannelError` — unified error surface for channel adapters.

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChannelError {
    /// `poll_events` / `send_message` called before `start()` (or
    /// after `stop()`).
    #[error("channel not started")]
    NotStarted,
    /// Platform-side auth failed — token expired, signature invalid,
    /// scope missing.
    #[error("auth failed: {0}")]
    Auth(String),
    /// Network / transport failure. Distinct from `Auth` so callers
    /// can retry transport but not auth.
    #[error("transport: {0}")]
    Transport(String),
    /// Config file missing / malformed.
    #[error("config: {0}")]
    Config(String),
    /// Platform rejected the request (e.g. malformed message).
    #[error("rejected by platform: {0}")]
    Rejected(String),
    /// Anything else — wrap with context.
    #[error("channel error: {0}")]
    Other(String),
}
