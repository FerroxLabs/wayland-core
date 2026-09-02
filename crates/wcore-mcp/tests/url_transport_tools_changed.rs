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

/// FerroxLabs/wayland#1213 c4, transport half.
///
/// c4 is explicit that `take_tools_changed` for this transport is only safe if
/// `is_alive`/`close` are fixed IN THE SAME CHANGE. They were — and nothing
/// graded them, which is how a fix on a line nobody runs survives.
///
/// The gate this protects is `McpManager::refresh_signalled_tools`, which
/// skips a server on `!transport.is_alive() || !transport.take_tools_changed()`
/// — `is_alive` first. Before #1175 this transport inherited
/// `is_alive() -> true` and had no notion of being closed, which was harmless
/// only while it also inherited `take_tools_changed() -> false`. With the
/// notification now reported and `is_alive` still stuck at `true`, a server
/// the operator removed would keep its tools re-registered on every
/// announcement: the resurrection the ticket names.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_streamable_http_transport_stops_reading_as_alive() {
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

    // POSITIVE CONTROL: an open transport reads alive and serves requests, or
    // the assertions after `close()` are satisfied by a transport that was
    // never working.
    assert!(
        transport.is_alive(),
        "a freshly connected transport must read as alive"
    );
    transport
        .request(&JsonRpcRequest::new(1, "initialize", None))
        .await
        .expect("initialize");
    assert!(
        settle(&transport, true).await,
        "control: this server does announce on its standalone stream, so the \
         post-close assertion is about `is_alive` and not about a silent server"
    );

    transport.close().await.expect("close");

    assert!(
        !transport.is_alive(),
        "a closed streamable-http transport still reads as alive, so \
         McpManager::refresh_signalled_tools would re-list and RE-REGISTER the \
         tools of a server the operator removed (FerroxLabs/wayland#1213 c4)"
    );
    let err = transport
        .request(&JsonRpcRequest::new(2, "tools/list", None))
        .await
        .expect_err("a closed transport must refuse further requests");
    assert!(
        err.to_string().contains("closed"),
        "unexpected error from a closed transport: {err}"
    );
}

/// A full SSE MCP server on loopback: it completes the real `initialize` /
/// `notifications/initialized` / `tools/list` handshake `McpManager` performs,
/// and lets the test push a server-initiated frame onto the same event stream
/// afterwards.
///
/// The transport-level tests above stop at `take_tools_changed()`. That is the
/// method `McpManager::refresh_signalled_tools` calls, but the criterion names
/// the MANAGER, so this fixture exists to grade the sentence end to end rather
/// than the one link of it that was edited.
struct SseMcpServer {
    url: String,
    to_client: tokio::sync::mpsc::UnboundedSender<String>,
    /// What the next `tools/list` will answer. The test edits this to model a
    /// server that registered a tool mid-session.
    tools: std::sync::Arc<std::sync::Mutex<String>>,
}

async fn spawn_full_sse_mcp_server(initial_tools: &str) -> SseMcpServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let tools = std::sync::Arc::new(std::sync::Mutex::new(initial_tools.to_string()));
    let tools_server = std::sync::Arc::clone(&tools);
    let (to_client, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(rx)));
    let sender = to_client.clone();

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let rx = std::sync::Arc::clone(&rx);
            let sender = sender.clone();
            let tools_server = std::sync::Arc::clone(&tools_server);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                // Read the request head one byte at a time so the body is not
                // swallowed with it; these requests are tiny.
                while !head.ends_with(b"\r\n\r\n") {
                    match socket.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => head.push(byte[0]),
                    }
                }
                let head = String::from_utf8_lossy(&head).to_string();

                if head.starts_with("GET") {
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                              Cache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n",
                        )
                        .await;
                    let body = "event: endpoint\ndata: /messages\n\n".to_string();
                    let _ = socket
                        .write_all(format!("{:x}\r\n{}\r\n", body.len(), body).as_bytes())
                        .await;
                    let _ = socket.flush().await;
                    let mut rx = match rx.lock().await.take() {
                        Some(rx) => rx,
                        // Only one event stream is expected; a second GET just
                        // parks so the connection is not refused.
                        None => return,
                    };
                    while let Some(frame) = rx.recv().await {
                        let body = format!("data: {frame}\n\n");
                        if socket
                            .write_all(format!("{:x}\r\n{}\r\n", body.len(), body).as_bytes())
                            .await
                            .is_err()
                        {
                            return;
                        }
                        let _ = socket.flush().await;
                    }
                    return;
                }

                let length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                if length > 0 && socket.read_exact(&mut body).await.is_err() {
                    return;
                }
                let _ = socket
                    .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = socket.flush().await;

                let request: serde_json::Value = match serde_json::from_slice(&body) {
                    Ok(value) => value,
                    Err(_) => return,
                };
                // A notification (no id) needs no reply — that is the whole
                // point of `notifications/initialized`.
                let Some(id) = request.get("id").and_then(|id| id.as_u64()) else {
                    return;
                };
                let result = match request.get("method").and_then(|m| m.as_str()) {
                    Some("initialize") => r#"{"protocolVersion":"2025-03-26","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"fixture","version":"0"}}"#.to_string(),
                    Some("tools/list") => tools_server
                        .lock()
                        .expect("fixture tool list is not poisoned")
                        .clone(),
                    _ => r#"{}"#.to_string(),
                };
                let _ = sender.send(format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#
                ));
            });
        }
    });

    SseMcpServer {
        url: format!("http://{addr}/sse"),
        to_client,
        tools,
    }
}

/// FerroxLabs/wayland#1213 c1, END TO END through the function the criterion
/// names.
///
/// "An SSE server's notifications/tools/list_changed reaches
/// `McpManager::refresh_signalled_tools`." The transport arms above prove the
/// nearer property — that `SseTransport::take_tools_changed()` rises — which is
/// the link that was edited. This one proves the SENTENCE: a real `McpManager`
/// built by the real `connect_all_with_policy` over a real loopback SSE server
/// re-issues `tools/list` for that server and returns it as refreshed, and the
/// tool the server registered mid-session is now in `all_tools()`. Before
/// #1213 the transport reported `false` forever and this returned an empty
/// vector no matter what the server said.
#[tokio::test(flavor = "multi_thread")]
async fn an_sse_servers_notification_reaches_manager_refresh_signalled_tools() {
    let server = spawn_full_sse_mcp_server(
        r#"{"tools":[{"name":"seed","description":"seeded at boot","inputSchema":{"type":"object"}}]}"#,
    )
    .await;

    let mut configs = HashMap::new();
    configs.insert(
        "hosted".to_string(),
        wcore_mcp::config::McpServerConfig {
            transport: wcore_mcp::config::TransportType::Sse,
            command: None,
            args: None,
            env: None,
            url: Some(server.url.clone()),
            headers: None,
            deferred: None,
            allow_local: true,
            only_for_assistant: None,
            allowed_tools: None,
        },
    );
    let manager = wcore_mcp::manager::McpManager::connect_all_with_policy(
        &configs,
        wcore_egress::default_policy(),
    )
    .await
    .expect("connect to the loopback SSE MCP server");

    // PRECONDITION: the handshake really happened. Without this the two
    // assertions below could both be satisfied by a manager holding no servers
    // at all.
    assert_eq!(
        manager.server_names(),
        vec!["hosted".to_string()],
        "the SSE handshake did not complete, so nothing below grades anything"
    );
    assert!(
        manager.has_tool_name("seed"),
        "boot tool list not discovered"
    );

    // NEGATIVE CONTROL: with nothing announced, the poll must be a no-op. A
    // transport that returned `true` unconditionally would refresh forever.
    assert!(
        manager.refresh_signalled_tools().await.is_empty(),
        "a silent server was refreshed anyway"
    );

    // The server registers a tool mid-session and announces it — id-less, the
    // shape the SSE listener used to discard. The re-issued `tools/list` is
    // answered by the fixture itself, from this updated list.
    *server
        .tools
        .lock()
        .expect("fixture tool list is not poisoned") =
        r#"{"tools":[{"name":"seed","description":"seeded at boot","inputSchema":{"type":"object"}},{"name":"late","description":"registered mid-session","inputSchema":{"type":"object"}}]}"#
            .to_string();
    server.to_client.send(TOOLS_CHANGED.to_string()).ok();

    let mut refreshed = Vec::new();
    for _ in 0..200 {
        refreshed = manager.refresh_signalled_tools().await;
        if !refreshed.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        refreshed,
        vec!["hosted".to_string()],
        "the SSE server's notifications/tools/list_changed never reached \
         McpManager::refresh_signalled_tools, so the tool it registered \
         mid-session stays uncallable for the life of the session \
         (FerroxLabs/wayland#1213 c1)"
    );
    assert!(
        manager.has_tool_name("late"),
        "the refresh ran but the mid-session tool was not adopted"
    );
}
