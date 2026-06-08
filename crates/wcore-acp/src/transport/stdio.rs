//! Stdio transport — JSON-Lines framing over stdin/stdout.
//!
//! Each line on the wire is one complete JSON object: a
//! [`JsonRpcRequest`] inbound or a [`JsonRpcResponse`]/[`MessageEvent`]
//! outbound. Used by Wayland's `acp serve --transport stdio` mode where
//! the client invokes the engine as a subprocess.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::error::AcpError;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse, MessageEvent};

/// One framed message inbound on the transport. Currently only
/// JSON-RPC requests arrive on stdio; clients that want to push
/// notifications use a different transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InboundFrame {
    Request(JsonRpcRequest),
}

/// One framed message outbound on the transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutboundFrame {
    Response(JsonRpcResponse),
    Event(MessageEvent),
}

/// Stdio transport — owns the reader half (single producer) and the
/// writer half (multi-producer via `Arc<Mutex<_>>` so streaming events
/// from background tasks can write concurrently).
pub struct StdioTransport<R, W>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    reader: BufReader<R>,
    writer: Arc<Mutex<W>>,
}

impl<R, W> StdioTransport<R, W>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    /// Construct a new stdio transport from a reader + writer pair.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer: Arc::new(Mutex::new(writer)),
        }
    }

    /// Get a writer handle that can be cloned and sent to background
    /// tasks for streaming events back to the peer.
    pub fn writer_handle(&self) -> Arc<Mutex<W>> {
        Arc::clone(&self.writer)
    }

    /// Read the next framed message from the transport. Returns
    /// `Ok(None)` on clean EOF.
    pub async fn recv(&mut self) -> Result<Option<InboundFrame>, AcpError> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .await
            .map_err(AcpError::Io)?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            // Skip blank keepalive lines.
            return Box::pin(self.recv()).await;
        }
        let frame: InboundFrame = serde_json::from_str(trimmed).map_err(AcpError::Serde)?;
        Ok(Some(frame))
    }

    /// Send a framed message. Acquires the writer mutex; concurrent
    /// senders interleave at message granularity (each line is atomic).
    pub async fn send(&self, frame: &OutboundFrame) -> Result<(), AcpError> {
        let mut line = serde_json::to_string(frame).map_err(AcpError::Serde)?;
        line.push('\n');
        let mut w = self.writer.lock().await;
        w.write_all(line.as_bytes()).await.map_err(AcpError::Io)?;
        w.flush().await.map_err(AcpError::Io)?;
        Ok(())
    }
}

/// Convenience constructor for the real stdio handles. Wires
/// `tokio::io::stdin()` and `tokio::io::stdout()` together so the
/// caller doesn't need to import them.
pub fn from_real_stdio() -> StdioTransport<tokio::io::Stdin, tokio::io::Stdout> {
    StdioTransport::new(tokio::io::stdin(), tokio::io::stdout())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{JSONRPC_VERSION, JsonRpcRequest, JsonRpcResponse};
    use tokio::io::{AsyncWriteExt, duplex};

    #[tokio::test]
    async fn roundtrip_request_response() {
        // client_w/client_r is the client side; server_w/server_r is the server side.
        let (client_side, server_side) = duplex(64);
        let (server_r, server_w) = tokio::io::split(server_side);
        let (mut client_r, mut client_w) = tokio::io::split(client_side);

        let mut srv = StdioTransport::new(server_r, server_w);

        // Client writes one JSON-RPC request line.
        let req = JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(1),
            method: "session/list".to_string(),
            params: None,
        };
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        client_w.write_all(line.as_bytes()).await.unwrap();
        client_w.flush().await.unwrap();

        // Server reads it.
        let got = srv.recv().await.unwrap().expect("inbound");
        match got {
            InboundFrame::Request(r) => assert_eq!(r.method, "session/list"),
        }

        // Server replies.
        let resp = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(1),
            result: Some(serde_json::json!({"sessions": []})),
            error: None,
        };
        srv.send(&OutboundFrame::Response(resp)).await.unwrap();

        // Client reads it.
        let mut buf = String::new();
        let mut buf_r = BufReader::new(&mut client_r);
        buf_r.read_line(&mut buf).await.unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(buf.trim_end()).unwrap();
        assert!(parsed.result.is_some());
    }

    #[tokio::test]
    async fn clean_eof_returns_none() {
        let (client_side, server_side) = duplex(8);
        let (server_r, server_w) = tokio::io::split(server_side);
        let mut srv = StdioTransport::new(server_r, server_w);
        drop(client_side); // EOF the server's reader
        let got = srv.recv().await.unwrap();
        assert!(got.is_none(), "expected clean EOF -> None");
    }

    #[tokio::test]
    async fn parse_error_surfaces_as_serde() {
        let (client_side, server_side) = duplex(64);
        let (server_r, server_w) = tokio::io::split(server_side);
        let (_client_r, mut client_w) = tokio::io::split(client_side);

        let mut srv = StdioTransport::new(server_r, server_w);
        client_w.write_all(b"not json\n").await.unwrap();
        let err = srv.recv().await.expect_err("expected serde error");
        assert!(matches!(err, AcpError::Serde(_)));
    }
}
