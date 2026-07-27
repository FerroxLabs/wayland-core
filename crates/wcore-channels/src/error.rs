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
    /// The operation is not part of this platform's surface at all.
    ///
    /// Deliberately DISTINCT from [`ChannelError::Rejected`]. "The platform
    /// refused this particular edit" and "this platform has no edit API" call
    /// for opposite operator responses — retry versus stop asking — and a
    /// contract operation that folded them together would let a caller retry
    /// forever against a surface that will never exist. Every contract
    /// operation with no honest default (`edit`, `delete`, `react`) returns
    /// this rather than a silent `Ok`, because a silent success is a caller
    /// believing a message was edited when nothing happened.
    #[error("{op} is unsupported on platform {platform}")]
    Unsupported { op: String, platform: String },
    /// Anything else — wrap with context.
    #[error("channel error: {0}")]
    Other(String),
}
