//! FerroxLabs/wayland#1175 c1 — a server attached at runtime must have its
//! `tools/list_changed` honoured, and MCP has THREE transports, not one.
//!
//! `McpTransport::take_tools_changed` defaults to `false`
//! (`transport/mod.rs`) and was overridden only by `StdioTransport`. So
//! `McpManager::refresh_signalled_tools` skipped every SSE and Streamable HTTP
//! server unconditionally: `/mcp add <url>` — the documented way to attach a
//! server mid-session, and the only shape a hosted MCP server can take —
//! announced a new tool, the announcement was discarded, and the tool stayed
//! uncallable for the rest of the session with nothing said.
//!
//! Each transport is graded here on its own real connect path against a real
//! loopback server, and each assertion is paired with a negative control so a
//! transport that returned `true` unconditionally would fail too.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wcore_mcp::protocol::JsonRpcRequest;
use wcore_mcp::transport::McpTransport;
use wcore_mcp::transport::sse::SseTransport;
use wcore_mcp::transport::streamable_http::StreamableHttpTransport;

const TOOLS_CHANGED: &str = r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#;
/// NEGATIVE CONTROL payload: a real MCP notification that is NOT the one the
/// manager acts on. A transport that sets its flag for any id-less frame would
/// pass the positive assertions and fail here.
const RESOURCES_CHANGED: &str =
    r#"{"jsonrpc":"2.0","method":"notifications/resources/list_changed"}"#;

/// Give the background listener task a chance to consume what the server
/// wrote. Polls rather than sleeping a fixed span so the test is not a race.
async fn settle(transport: &dyn McpTransport, want: bool) -> bool {
    for _ in 0..200 {
        if transport.take_tools_changed() {
            return true;
        }
        if !want {
            // Nothing to wait for beyond one full poll cycle in the negative
            // direction; keep going to give a buggy transport time to be wrong.
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// SSE MCP server on loopback: emits the `endpoint` event the handshake needs,
/// then whatever frames the caller asked for, then holds the socket open.
async fn spawn_sse_server(frames: &'static [&'static str]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                if request.starts_with("POST") {
                    let _ = socket
                        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    return;
                }
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                          Cache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n",
                    )
                    .await;
                let mut body = String::from("event: endpoint\ndata: /post\n\n");
                for frame in frames {
                    body.push_str(&format!("data: {frame}\n\n"));
                }
                let _ = socket
                    .write_all(format!("{:x}\r\n{}\r\n", body.len(), body).as_bytes())
                    .await;
                let _ = socket.flush().await;
                // Hold the stream open: a finished listener is not alive, and
                // the manager skips a dead transport before it ever asks.
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
        }
    });
    format!("http://{addr}/sse")
}

/// Streamable HTTP MCP server on loopback. `post_body` answers the POST;
/// `get_frames` are served on the standalone `GET` event stream.
async fn spawn_streamable_server(
    post_body: &'static str,
    post_content_type: &'static str,
    get_frames: &'static [&'static str],
    get_supported: bool,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                if request.starts_with("GET") {
                    if !get_supported {
                        let _ = socket
                            .write_all(
                                b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n",
                            )
                            .await;
                        return;
                    }
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                              Transfer-Encoding: chunked\r\n\r\n",
                        )
                        .await;
                    let mut body = String::new();
                    for frame in get_frames {
                        body.push_str(&format!("data: {frame}\n\n"));
                    }
                    let _ = socket
                        .write_all(format!("{:x}\r\n{}\r\n", body.len(), body).as_bytes())
                        .await;
                    let _ = socket.flush().await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    return;
                }
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {post_content_type}\r\n\
                             Mcp-Session-Id: sess-1175\r\nContent-Length: {}\r\n\r\n{post_body}",
                            post_body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                let _ = socket.flush().await;
            });
        }
    });
    format!("http://{addr}/mcp")
}

/// SSE: the event stream is the ONLY server-to-client channel this transport
/// has, and the notification carries no `id`, so the response-routing arm
/// discarded it.
#[tokio::test(flavor = "multi_thread")]
async fn an_sse_server_has_its_tools_list_changed_honoured() {
    let url = spawn_sse_server(&[TOOLS_CHANGED]).await;
    let transport = SseTransport::connect_with_timeout_and_policy(
        &url,
        &HashMap::new(),
        Duration::from_secs(5),
        true,
        wcore_egress::default_policy(),
    )
    .await
    .expect("connect to loopback SSE server");

    assert!(
        settle(&transport, true).await,
        "the SSE server announced tools/list_changed and the transport never \
         reported it — every tool it registers mid-session stays uncallable"
    );
    assert!(
        !transport.take_tools_changed(),
        "the flag must be take-and-CLEAR, or one notification refreshes forever"
    );
}

/// NEGATIVE CONTROL for the SSE arm: a different id-less notification must NOT
/// raise the flag.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrelated_sse_notification_does_not_signal_a_tool_change() {
    let url = spawn_sse_server(&[RESOURCES_CHANGED]).await;
    let transport = SseTransport::connect_with_timeout_and_policy(
        &url,
        &HashMap::new(),
        Duration::from_secs(5),
        true,
        wcore_egress::default_policy(),
    )
    .await
    .expect("connect to loopback SSE server");

    assert!(
        !settle(&transport, false).await,
        "resources/list_changed is not tools/list_changed"
    );
}

/// Streamable HTTP, channel 1: a notification interleaved in the SSE body of
/// the POST the server is answering. Two things are asserted at once, because
/// the same frame caused both failures: the notification must RAISE the flag,
/// and it must NOT be handed back as the reply — a notification has no `id`
/// and no `error`, so it deserialized cleanly into `JsonRpcResponse` and was
/// returned instead of the real response.
#[tokio::test(flavor = "multi_thread")]
async fn a_streamable_http_notification_in_a_response_stream_is_honoured_and_not_mistaken_for_the_reply()
 {
    let url = spawn_streamable_server(
        "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n\
         data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n\n",
        "text/event-stream",
        &[],
        false,
    )
    .await;
    let transport = StreamableHttpTransport::connect_with_policy(
        &url,
        &HashMap::new(),
        true,
        wcore_egress::default_policy(),
    )
    .await
    .expect("connect to loopback streamable-http server");

    let response = transport
        .request(&JsonRpcRequest::new(1, "tools/list", None))
        .await
        .expect("the real reply must survive the interleaved notification");
    assert_eq!(
        response.id,
        Some(1),
        "the notification frame was returned as the reply"
    );
    assert!(
        response.result.is_some(),
        "the notification frame was returned as the reply"
    );
    assert!(
        transport.take_tools_changed(),
        "the interleaved tools/list_changed was dropped"
    );
}

/// Streamable HTTP, channel 2: the standalone `GET` event stream. This is the
/// only channel a server has for an announcement made while no request is in
/// flight, and the transport never opened one.
#[tokio::test(flavor = "multi_thread")]
async fn a_streamable_http_server_has_its_standalone_stream_listened_to() {
    let url = spawn_streamable_server(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}",
        "application/json",
        &[TOOLS_CHANGED],
        true,
    )
    .await;
    let transport = StreamableHttpTransport::connect_with_policy(
        &url,
        &HashMap::new(),
        true,
        wcore_egress::default_policy(),
    )
    .await
    .expect("connect to loopback streamable-http server");

    transport
        .request(&JsonRpcRequest::new(1, "initialize", None))
        .await
        .expect("initialize");

    assert!(
        settle(&transport, true).await,
        "the server announced tools/list_changed on its standalone event \
         stream and the transport was not listening to it"
    );
}

/// NEGATIVE CONTROL for the standalone-stream arm: a server that answers the
/// GET with `405` (the spec's "no standalone stream here") must leave the flag
/// down rather than have the listener invent one.
#[tokio::test(flavor = "multi_thread")]
async fn a_streamable_http_server_without_a_standalone_stream_signals_nothing() {
    let url = spawn_streamable_server(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}",
        "application/json",
        &[TOOLS_CHANGED],
        false,
    )
    .await;
    let transport = StreamableHttpTransport::connect_with_policy(
        &url,
        &HashMap::new(),
        true,
        wcore_egress::default_policy(),
    )
    .await
    .expect("connect to loopback streamable-http server");

    transport
        .request(&JsonRpcRequest::new(1, "initialize", None))
        .await
        .expect("initialize");

    assert!(
        !settle(&transport, false).await,
        "a 405 GET must not raise the tools-changed flag"
    );
}
