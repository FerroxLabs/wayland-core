pub mod sse;
pub mod stdio;
pub mod stdio_readiness;
pub mod streamable_http;

use async_trait::async_trait;

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// The MCP notification a server sends when its tool list changes.
pub(crate) const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";

/// Is this id-less inbound frame the `tools/list_changed` notification?
///
/// One implementation for every transport (#1175): the fix that introduced
/// this predicate lived inside `stdio.rs`, so SSE and Streamable-HTTP had no
/// way to spell the same question and silently ignored the notification.
///
/// Parsed as a generic JSON value rather than a typed notification struct:
/// the only field that matters is `method`, and a malformed or unrelated
/// notification must be a plain `false`, never an error that kills the reader.
pub(crate) fn notified_tools_changed(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("method")
                .and_then(|method| method.as_str())
                .map(|method| method == TOOLS_LIST_CHANGED)
        })
        .unwrap_or(false)
}

/// Transport abstraction for MCP communication
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and receive the response
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;

    /// Send a notification (no response expected)
    async fn notify(&self, req: &JsonRpcRequest) -> Result<(), McpError>;

    /// Close the transport
    async fn close(&self) -> Result<(), McpError>;

    /// Take-and-clear the "the server told us its tool list changed" flag.
    ///
    /// MCP servers may register or drop tools mid-session and announce it
    /// with `notifications/tools/list_changed` (declared via the
    /// `tools.listChanged` capability). That notification carries no `id`,
    /// so it is not a response to any request and can only be observed by
    /// whatever owns the inbound stream — the transport. This is how the
    /// manager learns to re-issue `tools/list`.
    ///
    /// Returns `true` at most once per notification burst: the flag is
    /// cleared by reading it, so a poller cannot re-refresh forever off one
    /// signal. Transports that do not observe server-initiated
    /// notifications always return `false`.
    fn take_tools_changed(&self) -> bool {
        false
    }

    /// Open the transport's serverâclient notification channel, if it has one
    /// that can only be opened after the MCP handshake.
    ///
    /// Called by [`McpManager`](crate::manager::McpManager) once, immediately
    /// after `notifications/initialized`. Transports whose inbound stream
    /// already exists by then (stdio's stdout reader, SSE's event stream) need
    /// nothing here and keep the default no-op. Streamable-HTTP overrides it:
    /// the MCP spec's standalone `GET` SSE stream is its ONLY channel for a
    /// server-initiated message when no request is in flight, and it cannot be
    /// opened before the session id the handshake assigns is known.
    ///
    /// Must never fail the session: a server may answer the standalone stream
    /// with `405 Method Not Allowed`, which the spec explicitly permits.
    async fn start_notification_stream(&self) {}

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

    #[error("MCP connect timed out after {after:?}{cleanup}")]
    ConnectTimedOut {
        after: std::time::Duration,
        cleanup: String,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// FerroxLabs/wayland#1137 — the pre-spawn malware gate refused the
    /// launch. Its own variant, not an `InitFailed`, so a host can render a
    /// supply-chain refusal differently from a server that merely failed to
    /// start: one is a security decision the user should see, the other is an
    /// operational error they can retry.
    #[error("MCP server launch refused: {0}")]
    MalwareBlocked(String),
}
