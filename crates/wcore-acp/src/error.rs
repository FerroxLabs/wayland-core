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
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}
