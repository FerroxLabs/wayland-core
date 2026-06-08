//! ACP — Agent Client Protocol — server + client implementation.
//!
//! Spec reference: https://github.com/anthropics/agent-client-protocol
//!
//! 1.A.1 — scaffold. 1.A.2 — protocol types. 1.A.3 — stdio transport.
//! 1.A.4 — HTTP/SSE transport. 1.A.5 — WebSocket transport.
//! 1.A.6 — minimal in-memory server. 1.A.8 — auth layer.
//! Client 1.A.7 + engine integration 1.A.10 still to land.
pub mod a2a;
pub mod auth;
pub mod client;
pub mod error;
pub mod protocol;
pub mod server;
pub mod transport;
pub mod turn;

pub use a2a::{A2aCapabilities, A2aError, A2aHandler, A2aHandshake, A2aMessage, DefaultA2aHandler};
pub use client::AcpClient;
pub use error::AcpError;
pub use server::AcpServer;
pub use turn::{TurnEngine, TurnRequest};

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AcpError>;
