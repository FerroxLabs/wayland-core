pub mod sse;
pub mod stdio;
pub mod streamable_http;

use async_trait::async_trait;

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Transport abstraction for MCP communication
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and receive the response
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;

    /// Send a notification (no response expected)
    async fn notify(&self, req: &JsonRpcRequest) -> Result<(), McpError>;

    /// Close the transport
    async fn close(&self) -> Result<(), McpError>;

    /// Whether the transport is still believed to be usable.
    ///
    /// Audit C4/C7: a server that dies (child process exits) or that the
    /// engine deliberately tears down on a cancelled wedged call should
    /// stop being treated as live, so the manager can prune it and stop
    /// advertising its tools. Transports without a backing process
    /// (HTTP-style) are always considered live — each request is
    /// independent and self-bounded by its own timeout.
    fn is_alive(&self) -> bool {
        true
    }
}

/// Errors from MCP transport and protocol
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Transport error: {0}")]
    Transport(String),

    #[error("JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String },

    #[error("Server not found: {0}")]
    ServerNotFound(String),

    #[error("Tool not found: {server}/{tool}")]
    ToolNotFound { server: String, tool: String },

    #[error("Initialization failed: {0}")]
    InitFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
