//! WebSocket transport — JSON text frames carrying JSON-RPC.
//!
//! Each WebSocket text frame is one complete JSON object: a
//! [`JsonRpcRequest`] inbound or a [`JsonRpcResponse`]/[`MessageEvent`]
//! outbound. Mirrors the stdio framing semantics but rides on a
//! WebSocket connection so browser clients and remote ACP peers can
//! talk to the engine.
//!
//! This transport wraps a single already-upgraded `WebSocketStream`.
//! A higher layer (the ACP server, 1.A.6) is responsible for accepting
//! incoming TCP connections, performing the HTTP Upgrade handshake,
//! and spawning one [`WsTransport`] per connection.

use std::sync::Arc;

use futures::{SinkExt, StreamExt, stream::SplitSink, stream::SplitStream};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

use crate::error::AcpError;
use crate::transport::stdio::{InboundFrame, OutboundFrame};

/// Convenience alias for the most common client-side stream type.
pub type ClientWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// WebSocket transport — owns the read half (single consumer) and the
/// write half (multi-producer via `Arc<Mutex<_>>` so streaming events
/// from background tasks can fan out concurrently).
///
/// Generic over `S: StreamExt + SinkExt<Message>` so both server-side
/// (`WebSocketStream<TcpStream>`) and client-side
/// (`WebSocketStream<MaybeTlsStream<TcpStream>>`) connections plug in
/// without duplication.
pub struct WsTransport<S>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin
        + Send
        + 'static,
{
    reader: SplitStream<S>,
    writer: Arc<Mutex<SplitSink<S, Message>>>,
}

impl<S> WsTransport<S>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin
        + Send
        + 'static,
{
    /// Construct a new WS transport from an already-upgraded WebSocket
    /// stream. Splits it into read + write halves so background tasks
    /// can stream events without contending with the recv loop.
    pub fn new(stream: S) -> Self {
        let (sink, source) = stream.split();
        Self {
            reader: source,
            writer: Arc::new(Mutex::new(sink)),
        }
    }

    /// Get a writer handle that can be cloned and sent to background
    /// tasks for streaming events back to the peer.
    pub fn writer_handle(&self) -> Arc<Mutex<SplitSink<S, Message>>> {
        Arc::clone(&self.writer)
    }

    /// Read the next framed message from the transport. Returns
    /// `Ok(None)` on clean close. Pings/pongs/binary frames are
    /// transparently skipped; only text frames carry protocol data.
    pub async fn recv(&mut self) -> Result<Option<InboundFrame>, AcpError> {
        loop {
            let msg = match self.reader.next().await {
                Some(Ok(m)) => m,
                Some(Err(e)) => {
                    return Err(AcpError::Transport(format!("ws read: {e}")));
                }
                None => return Ok(None),
            };
            match msg {
                Message::Text(text) => {
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let frame: InboundFrame =
                        serde_json::from_str(trimmed).map_err(AcpError::Serde)?;
                    return Ok(Some(frame));
                }
                Message::Close(_) => return Ok(None),
                // Ignore ping/pong/binary — protocol rides text frames.
                Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {
                    continue;
                }
            }
        }
    }

    /// Send a framed message as a single WebSocket text frame.
    /// Acquires the writer mutex; concurrent senders interleave at
    /// message granularity (each frame is atomic).
    pub async fn send(&self, frame: &OutboundFrame) -> Result<(), AcpError> {
        let line = serde_json::to_string(frame).map_err(AcpError::Serde)?;
        let mut w = self.writer.lock().await;
        w.send(Message::Text(line))
            .await
            .map_err(|e| AcpError::Transport(format!("ws send: {e}")))?;
        Ok(())
    }

    /// Send a clean close frame to the peer. Best-effort.
    pub async fn close(&self) -> Result<(), AcpError> {
        let mut w = self.writer.lock().await;
        w.send(Message::Close(None))
            .await
            .map_err(|e| AcpError::Transport(format!("ws close: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{JSONRPC_VERSION, JsonRpcRequest, JsonRpcResponse};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{accept_async, client_async};

    async fn pair() -> (
        WsTransport<WebSocketStream<TcpStream>>,
        WsTransport<WebSocketStream<TcpStream>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = accept_async(stream).await.unwrap();
            WsTransport::new(ws)
        });

        let url = format!("ws://{addr}/");
        let client_stream = TcpStream::connect(addr).await.unwrap();
        let (client_ws, _resp) = client_async(url, client_stream).await.unwrap();
        let client = WsTransport::new(client_ws);
        let server = server_task.await.unwrap();
        (server, client)
    }

    #[tokio::test]
    async fn roundtrip_request_response() {
        let (mut server, client) = pair().await;

        let req = JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(1),
            method: "session/list".to_string(),
            params: None,
        };

        // Client sends a request frame → server receives.
        // InboundFrame is untagged with a single Request variant, so
        // serializing the request directly produces a parseable frame.
        let line = serde_json::to_string(&req).unwrap();
        {
            let handle = client.writer_handle();
            let mut w = handle.lock().await;
            w.send(Message::Text(line)).await.unwrap();
        }

        let got = server.recv().await.unwrap().expect("inbound");
        match got {
            InboundFrame::Request(r) => assert_eq!(r.method, "session/list"),
        }

        // Server sends a response frame back via the typed send() path.
        server
            .send(&OutboundFrame::Response(JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: serde_json::json!(1),
                result: Some(serde_json::json!({"ok": true})),
                error: None,
            }))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn clean_close_returns_none() {
        let (mut server, client) = pair().await;
        client.close().await.unwrap();
        let got = server.recv().await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn parse_error_surfaces_as_serde() {
        let (mut server, client) = pair().await;
        {
            let handle = client.writer_handle();
            let mut w = handle.lock().await;
            w.send(Message::Text("not json".into())).await.unwrap();
        }
        let err = server.recv().await.expect_err("expected serde error");
        assert!(matches!(err, AcpError::Serde(_)));
    }
}
