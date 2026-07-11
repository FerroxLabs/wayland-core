//! Crate-wide error type.
use thiserror::Error;

/// Errors that can occur during ACP server / client operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AcpError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("authentication error: {0}")]
    Auth(String),
    #[error("session error: {0}")]
    Session(String),
    /// persona-profiles Phase A (red-team R3/R4): a `session/create` named an
    /// `agent` selector the calling principal is NOT authorized for (or that
    /// does not exist). Kept distinct from [`Self::Session`] so the transport
    /// maps it to [`crate::protocol::ErrorCode::AgentNotFound`]. Because the
    /// roster returns only AUTHORIZED agents, "unknown" and "not authorized"
    /// are the SAME signal — this leaks no existence information.
    #[error("agent error: {0}")]
    Agent(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}
