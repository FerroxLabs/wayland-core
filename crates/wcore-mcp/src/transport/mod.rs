pub mod sse;
pub mod stdio;
pub mod stdio_readiness;
pub mod streamable_http;

use async_trait::async_trait;

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// The MCP notification a server sends when its tool list changes.
pub(crate) const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";

/// Is this inbound frame the `tools/list_changed` notification?
///
/// Parsed as a generic JSON value rather than a typed notification struct:
/// the only field that matters is `method`, and a malformed or unrelated
/// notification must be a plain `false`, never an error that kills the reader.
///
/// FerroxLabs/wayland#1175 — lives here rather than in `stdio` because all
/// THREE transports need it. It was stdio-private while `take_tools_changed`
/// was stdio-only, which is exactly the defect: a server attached over SSE or
/// Streamable HTTP had its announcement discarded for the life of the session.
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

#[cfg(test)]
mod tests {
    /// FerroxLabs/wayland#1175 — `take_tools_changed` has a trait DEFAULT of
    /// `false`, and for two of the three transports nobody overrode it. That
    /// default is silent: an SSE or Streamable HTTP server announced a tool and
    /// `McpManager::refresh_signalled_tools` skipped it for the life of the
    /// session, with no warning anywhere.
    ///
    /// A behavioural test per transport cannot catch the FOURTH transport
    /// somebody adds next year, so this grades the class: every
    /// `impl McpTransport` in this module tree must say what it does about
    /// server-initiated tool-list changes.
    #[test]
    fn every_transport_decides_take_tools_changed_for_itself() {
        const SOURCES: [(&str, &str); 3] = [
            ("stdio", include_str!("stdio.rs")),
            ("sse", include_str!("sse.rs")),
            ("streamable_http", include_str!("streamable_http.rs")),
        ];

        for (name, source) in SOURCES {
            // POSITIVE CONTROL: prove the file really is a transport impl
            // before reading anything into the absence below.
            assert!(
                source.contains("impl McpTransport for"),
                "{name} no longer implements McpTransport — this lint is \
                 grading the wrong file"
            );
            assert!(
                source.contains("fn take_tools_changed"),
                "{name} inherits the `false` default for take_tools_changed, so \
                 a tools/list_changed it receives is discarded and the tool \
                 stays uncallable for the session (FerroxLabs/wayland#1175)"
            );
        }
    }
}
