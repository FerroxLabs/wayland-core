//! T2-E1: SSE (Server-Sent Events) transport.
//!
//! Minimal HTTP listener implementing the MCP SSE binding: clients
//! POST JSON-RPC requests, we respond `text/event-stream` framed
//! responses (`event: message\ndata: <json>\n\n`).
//!
//! ## Why raw `tokio::net` instead of axum/hyper
//!
//! `wcore-mcp`'s `Cargo.toml` does not pull in `axum` or `hyper`, and
//! the T2-E1 brief forbids adding heavy new deps. The HTTP surface we
//! need is small enough (single endpoint, POST with body, single
//! event back) that a hand-rolled parser using `tokio::net::TcpListener`
//! plus `BufReader` stays well under 300 LOC and avoids dragging axum's
//! type system into a mid-layer crate.
//!
//! ## Protocol shape
//!
//! - Request: `POST / HTTP/1.1` with `Content-Length: N` + JSON-RPC body.
//! - Response: `200 OK` with `Content-Type: text/event-stream` and a
//!   single SSE event carrying the JSON-RPC response. Connection
//!   closes after the event (one-shot — this is sufficient for v0.6.2;
//!   streaming multi-event sessions land in a later wave).
//! - Anything else: `405` for non-POST, `400` for malformed.
//!
//! Single-connection limits — body size capped at 1 MiB to stop a
//! pathological peer from exhausting memory.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::server::{McpServer, ServerJsonRpcRequest, ServerJsonRpcResponse, error_code};

const MAX_BODY_BYTES: usize = 1024 * 1024;

/// SSE listener config. Default binds to `127.0.0.1:9876` (local only).
#[derive(Debug, Clone)]
pub struct SseConfig {
    pub bind: SocketAddr,
}

impl Default for SseConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9876),
        }
    }
}

/// Bind a listener and serve forever. For graceful-shutdown variants,
/// callers can drive `bind_listener` + their own accept loop.
pub async fn serve_sse(server: McpServer, cfg: SseConfig) -> std::io::Result<()> {
    let listener = TcpListener::bind(cfg.bind).await?;
    accept_loop(Arc::new(server), listener).await
}

/// Lower-level: bind only, return both the listener and the actual
/// bound address (useful when `cfg.bind` uses port 0). Exposed for
/// tests that want to know the ephemeral port.
pub async fn bind_listener(cfg: &SseConfig) -> std::io::Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind(cfg.bind).await?;
    let addr = listener.local_addr()?;
    Ok((listener, addr))
}

/// Run the accept loop against an already-bound listener.
pub async fn accept_loop(server: Arc<McpServer>, listener: TcpListener) -> std::io::Result<()> {
    loop {
        let (stream, _peer) = listener.accept().await?;
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            // Per-connection errors are logged-and-dropped: a single
            // bad client must not kill the server.
            let _ = handle_connection(server, stream).await;
        });
    }
}

/// Handle one HTTP connection: parse the request, dispatch via the
/// server, emit a single SSE event, close.
pub async fn handle_connection(
    server: Arc<McpServer>,
    mut stream: TcpStream,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);

    // ---- Request line ----
    let mut request_line = String::new();
    let n = reader.read_line(&mut request_line).await?;
    if n == 0 {
        return Ok(());
    }
    let request_line = request_line.trim_end_matches(['\r', '\n']);
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let _target = parts.next().unwrap_or("");

    if method != "POST" {
        return write_simple_response(
            &mut writer,
            405,
            "Method Not Allowed",
            "text/plain; charset=utf-8",
            b"only POST is supported\n",
        )
        .await;
    }

    // ---- Headers ----
    let mut content_length: Option<usize> = None;
    let mut content_length_invalid = false;
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header).await?;
        if n == 0 {
            break;
        }
        let trimmed = header.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        // Case-insensitive header name match.
        if let Some(colon) = trimmed.find(':') {
            let (name, value) = trimmed.split_at(colon);
            let value = value[1..].trim();
            if name.eq_ignore_ascii_case("content-length") {
                match value.trim().parse::<usize>() {
                    Ok(v) => content_length = Some(v),
                    Err(_) => content_length_invalid = true,
                }
            }
        }
    }

    if content_length_invalid {
        return write_simple_response(
            &mut writer,
            400,
            "Bad Request",
            "text/plain; charset=utf-8",
            b"invalid Content-Length\n",
        )
        .await;
    }
    let content_length = match content_length {
        Some(v) => v,
        None => {
            return write_simple_response(
                &mut writer,
                400,
                "Bad Request",
                "text/plain; charset=utf-8",
                b"missing Content-Length header\n",
            )
            .await;
        }
    };
    if content_length == 0 {
        return write_simple_response(
            &mut writer,
            400,
            "Bad Request",
            "text/plain; charset=utf-8",
            b"zero Content-Length\n",
        )
        .await;
    }
    if content_length > MAX_BODY_BYTES {
        return write_simple_response(
            &mut writer,
            413,
            "Payload Too Large",
            "text/plain; charset=utf-8",
            b"body exceeds 1 MiB cap\n",
        )
        .await;
    }

    // ---- Body ----
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await?;

    // ---- Dispatch ----
    let response = match serde_json::from_slice::<ServerJsonRpcRequest>(&body) {
        Ok(req) => server.handle_request(req).await,
        Err(e) => {
            ServerJsonRpcResponse::err(None, error_code::PARSE_ERROR, format!("parse error: {}", e))
        }
    };
    let response_json = serde_json::to_string(&response).map_err(std::io::Error::other)?;

    // ---- Reply: single SSE event ----
    let event = format!("event: message\ndata: {}\n\n", response_json);
    write_simple_response(
        &mut writer,
        200,
        "OK",
        "text/event-stream; charset=utf-8",
        event.as_bytes(),
    )
    .await
}

/// Write a complete (non-chunked) HTTP/1.1 response and close.
async fn write_simple_response<W>(
    writer: &mut W,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let header = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         \r\n",
        status,
        reason,
        content_type,
        body.len()
    );
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::default_tool_set;
    use serde_json::{Value, json};
    use tokio::net::TcpStream;

    /// Tiny HTTP client: POST a JSON body, return (status, body).
    /// Strips chunked-encoding handling — we know our server always
    /// sends Content-Length. SSE body is returned verbatim so tests
    /// can parse the `data:` line.
    async fn post_json(addr: SocketAddr, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        let request = format!(
            "POST / HTTP/1.1\r\n\
             Host: {}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n{}",
            addr,
            body.len(),
            body
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write request");
        stream.flush().await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read response");
        let text = String::from_utf8(buf).expect("utf8");
        let mut parts = text.splitn(2, "\r\n\r\n");
        let head = parts.next().unwrap_or("");
        let body = parts.next().unwrap_or("");
        let status_line = head.lines().next().unwrap_or("");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, body.to_string())
    }

    /// Pull the `data: ...` JSON payload out of an SSE body.
    fn extract_sse_data(body: &str) -> Value {
        let data_line = body
            .lines()
            .find(|l| l.starts_with("data: "))
            .expect("data line present");
        let json_str = &data_line["data: ".len()..];
        serde_json::from_str(json_str).expect("json")
    }

    #[test]
    fn sse_config_default_port_is_9876() {
        let cfg = SseConfig::default();
        assert_eq!(cfg.bind.port(), 9876);
        assert!(cfg.bind.ip().is_loopback());
    }

    #[tokio::test]
    async fn sse_listener_binds_and_returns_addr() {
        let cfg = SseConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
        };
        let (_listener, addr) = bind_listener(&cfg).await.expect("bind");
        assert!(addr.port() > 0, "ephemeral port assigned");
        assert!(addr.ip().is_loopback());
    }

    /// Helper: spin up a server on an ephemeral port; return addr +
    /// abort handle.
    async fn spawn_server(server: McpServer) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let cfg = SseConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
        };
        let (listener, addr) = bind_listener(&cfg).await.expect("bind");
        let server = Arc::new(server);
        let handle = tokio::spawn(async move {
            let _ = accept_loop(server, listener).await;
        });
        (addr, handle)
    }

    /// R2 fix A3: default tool set is empty in v0.6.2 — stubs no longer
    /// advertised via `tools/list` (MCP spec compliance).
    #[tokio::test]
    async fn sse_tools_list_over_http_returns_default_set() {
        let server = McpServer::with_defaults();
        let (addr, handle) = spawn_server(server).await;
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string();
        let (status, sse_body) = post_json(addr, &body).await;
        assert_eq!(status, 200);
        let v = extract_sse_data(&sse_body);
        let tools = v["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 0);
        handle.abort();
    }

    #[tokio::test]
    async fn sse_unknown_method_returns_method_not_found() {
        let server = McpServer::with_defaults();
        let (addr, handle) = spawn_server(server).await;
        let body = json!({"jsonrpc": "2.0", "id": 2, "method": "bogus"}).to_string();
        let (status, sse_body) = post_json(addr, &body).await;
        assert_eq!(status, 200);
        let v = extract_sse_data(&sse_body);
        assert_eq!(v["error"]["code"], error_code::METHOD_NOT_FOUND);
        handle.abort();
    }

    #[tokio::test]
    async fn sse_request_id_preserved() {
        let server = McpServer::with_defaults();
        let (addr, handle) = spawn_server(server).await;
        let body = json!({"jsonrpc": "2.0", "id": "client-99", "method": "tools/list"}).to_string();
        let (_status, sse_body) = post_json(addr, &body).await;
        let v = extract_sse_data(&sse_body);
        assert_eq!(v["id"], json!("client-99"));
        handle.abort();
    }

    #[tokio::test]
    async fn sse_concurrent_clients_both_get_responses() {
        let server = McpServer::new(default_tool_set(), Box::new(crate::server::AllowAll));
        let (addr, handle) = spawn_server(server).await;
        let body_a = json!({"jsonrpc": "2.0", "id": 100, "method": "initialize"}).to_string();
        let body_b = json!({"jsonrpc": "2.0", "id": 200, "method": "tools/list"}).to_string();

        let fut_a = post_json(addr, &body_a);
        let fut_b = post_json(addr, &body_b);
        let (resp_a, resp_b) = tokio::join!(fut_a, fut_b);

        assert_eq!(resp_a.0, 200);
        assert_eq!(resp_b.0, 200);

        let va = extract_sse_data(&resp_a.1);
        let vb = extract_sse_data(&resp_b.1);
        assert_eq!(va["id"], json!(100));
        assert_eq!(vb["id"], json!(200));
        // initialize result vs tools/list result distinguishable:
        assert!(va["result"]["protocolVersion"].is_string());
        assert!(vb["result"]["tools"].is_array());
        handle.abort();
    }
}
