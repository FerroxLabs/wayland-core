//! #1175 — `notifications/tools/list_changed` on the NON-stdio transports.
//!
//! #1175 was closed with a fix that reached StdioTransport only:
//! `McpTransport::take_tools_changed()` defaults to `false` and was overridden
//! nowhere else. `SseTransport`'s listener dropped every id-less frame (`&& let
//! Some(id) = response.id`) — precisely the shape of a JSON-RPC notification —
//! and `StreamableHttpTransport` treated `text/event-stream` purely as the
//! framing of a reply to its own request. `McpManager::refresh_signalled_tools`
//! therefore skipped both unconditionally: a hosted MCP server attached with
//! `/mcp add <url>` announced a new tool, the announcement was discarded, and
//! the tool stayed uncallable for the life of the session with no warning.
//!
//! These tests drive the REAL transports against loopback servers, so they hold
//! against the pre-fix source unchanged.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use wcore_mcp::protocol::JsonRpcRequest;
use wcore_mcp::transport::McpTransport;
use wcore_mcp::transport::sse::SseTransport;
use wcore_mcp::transport::streamable_http::StreamableHttpTransport;

/// Poll `take_tools_changed` for up to a second so the assertion does not race
/// the background listener.
async fn await_tools_changed(transport: &dyn McpTransport) -> bool {
    for _ in 0..100 {
        if transport.take_tools_changed() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// A loopback SSE MCP server: emits the `endpoint` event, then whatever event
/// blocks the caller asked for, then holds the connection open.
fn spawn_sse_server(events: &'static str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    listener.set_nonblocking(true).expect("nonblocking");
    let listener = TcpListener::from_std(listener).expect("from_std");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                if head.starts_with("GET") {
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
                              event: endpoint\ndata: /messages\n\n",
                        )
                        .await;
                    let _ = socket.write_all(events.as_bytes()).await;
                    let _ = socket.flush().await;
                    // Hold the stream open so the listener is not racing an EOF.
                    tokio::time::sleep(Duration::from_secs(30)).await;
                } else {
                    let _ = socket
                        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                        .await;
                }
            });
        }
    });
    format!("http://{addr}/sse")
}

/// THE DEFECT, SSE arm. The server announces `tools/list_changed` on the very
/// stream the transport already owns; the client must observe it.
#[tokio::test]
async fn sse_transport_observes_tools_list_changed() {
    let url = spawn_sse_server(concat!(
        "event: message\ndata: ",
        r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
        "\n\n"
    ));
    let transport = SseTransport::connect(&url, &HashMap::new(), true)
        .await
        .expect("SSE connect");

    assert!(
        await_tools_changed(&transport).await,
        "the server announced tools/list_changed on the SSE stream and the \
         client must see it — otherwise every tool it registers mid-session \
         stays uncallable for the life of the session"
    );
    assert!(
        !transport.take_tools_changed(),
        "the flag is take-and-cleared: one announcement must not refresh forever"
    );
}

/// NEGATIVE CONTROL — must hold in BOTH arms. Only this one method may raise
/// the flag; a resources notification, an unrelated notification and a plain
/// response must not.
#[tokio::test]
async fn sse_transport_ignores_every_other_id_less_frame() {
    let url = spawn_sse_server(concat!(
        "event: message\ndata: ",
        r#"{"jsonrpc":"2.0","method":"notifications/resources/list_changed"}"#,
        "\n\n",
        "event: message\ndata: ",
        r#"{"jsonrpc":"2.0","method":"log/info"}"#,
        "\n\n",
        "event: message\ndata: ",
        r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#,
        "\n\n",
    ));
    let transport = SseTransport::connect(&url, &HashMap::new(), true)
        .await
        .expect("SSE connect");

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !transport.take_tools_changed(),
        "only notifications/tools/list_changed may raise the flag"
    );
}

/// A loopback Streamable-HTTP MCP server.
///
/// `post_body` answers every POST (as `text/event-stream`). A GET with
/// `Accept: text/event-stream` — the MCP spec's standalone server→client
/// channel — is answered with `get_body` when `serve_get` is set, and with the
/// spec-legal `405 Method Not Allowed` when it is not.
fn spawn_http_server(
    post_body: &'static str,
    serve_get: bool,
    get_body: &'static str,
) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    listener.set_nonblocking(true).expect("nonblocking");
    let listener = TcpListener::from_std(listener).expect("from_std");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                if head.starts_with("GET") {
                    if serve_get {
                        let _ = socket
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n",
                            )
                            .await;
                        loop {
                            if socket.write_all(get_body.as_bytes()).await.is_err() {
                                return;
                            }
                            let _ = socket.flush().await;
                            tokio::time::sleep(Duration::from_millis(25)).await;
                        }
                    } else {
                        let _ = socket
                            .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")
                            .await;
                    }
                } else {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
                        post_body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(post_body.as_bytes()).await;
                    let _ = socket.flush().await;
                }
            });
        }
    });
    format!("http://{addr}/mcp")
}

/// THE DEFECT, Streamable-HTTP arm 1. A server may interleave a notification
/// into the SSE framing of a response it is already streaming. That frame was
/// classified `Skip` and thrown away.
#[tokio::test]
async fn streamable_http_observes_a_notification_inside_a_response_stream() {
    let url = spawn_http_server(
        concat!(
            "data: ",
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
            "\n\n",
            "data: ",
            r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#,
            "\n\n"
        ),
        false,
        "",
    );
    let transport = StreamableHttpTransport::connect(&url, &HashMap::new(), true)
        .await
        .expect("streamable-http connect");

    let response = transport
        .request(&JsonRpcRequest::new(1, "tools/list", None))
        .await
        .expect("the response frame must still be returned");
    assert!(response.result.is_some(), "the response must survive");

    assert!(
        transport.take_tools_changed(),
        "a tools/list_changed frame interleaved in the response stream must be \
         observed, not discarded"
    );
    assert!(
        !transport.take_tools_changed(),
        "the flag is take-and-cleared"
    );
}

/// THE DEFECT, Streamable-HTTP arm 2. The spec's standalone `GET` SSE stream is
/// how a server delivers a notification when no request is in flight — the case
/// the ticket actually describes (a server attached mid-session announces a new
/// tool). The transport opened no such stream at all.
#[tokio::test]
async fn streamable_http_observes_the_standalone_notification_stream() {
    let url = spawn_http_server(
        concat!("data: ", r#"{"jsonrpc":"2.0","id":1,"result":{}}"#, "\n\n"),
        true,
        concat!(
            "data: ",
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
            "\n\n"
        ),
    );
    let transport = StreamableHttpTransport::connect(&url, &HashMap::new(), true)
        .await
        .expect("streamable-http connect");
    transport.start_notification_stream().await;

    assert!(
        await_tools_changed(&transport).await,
        "the standalone GET SSE stream is the MCP spec's server→client channel; \
         a list_changed announced there must be observed"
    );
}

/// NEGATIVE CONTROL — a server that refuses the standalone GET with the
/// spec-legal `405` must not break anything: connect succeeds, requests still
/// work, and no phantom flag is raised.
#[tokio::test]
async fn a_server_that_refuses_the_standalone_stream_still_works() {
    let url = spawn_http_server(
        concat!("data: ", r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#, "\n\n"),
        false,
        "",
    );
    let transport = StreamableHttpTransport::connect(&url, &HashMap::new(), true)
        .await
        .expect("streamable-http connect");
    transport.start_notification_stream().await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let response = transport
        .request(&JsonRpcRequest::new(1, "tools/list", None))
        .await
        .expect("a 405 on the standalone stream must not break requests");
    assert!(response.result.is_some());
    assert!(
        !transport.take_tools_changed(),
        "no announcement was made, so no flag may be raised"
    );
}

/// Audit follow-on named by the same finding: `StreamableHttpTransport`
/// inherited `is_alive() -> true` and its `close()` could not make it false.
/// With `take_tools_changed` now implemented there, that combination would let
/// an operator-removed server's tools be re-registered on its next
/// `list_changed`, so the liveness signal has to be real.
#[tokio::test]
async fn streamable_http_close_makes_the_transport_not_alive() {
    let url = spawn_http_server(
        concat!("data: ", r#"{"jsonrpc":"2.0","id":1,"result":{}}"#, "\n\n"),
        true,
        concat!(
            "data: ",
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
            "\n\n"
        ),
    );
    let transport = StreamableHttpTransport::connect(&url, &HashMap::new(), true)
        .await
        .expect("streamable-http connect");
    assert!(transport.is_alive(), "a fresh transport must be alive");

    transport.close().await.expect("close");
    assert!(
        !transport.is_alive(),
        "a closed transport must stop being treated as live, or the manager \
         keeps advertising a removed server's tools"
    );
}

// ---------------------------------------------------------------------------
// End to end, through the manager
// ---------------------------------------------------------------------------

/// A loopback SSE MCP server that speaks the real handshake.
///
/// One `GET` establishes the event stream; every `POST` is answered by writing
/// a `message` event back onto that stream, which is how the SSE transport
/// binding actually works. After the first `tools/list` the server announces
/// `notifications/tools/list_changed` and starts answering `tools/list` with a
/// SECOND tool — the mid-session registration the ticket is about.
fn spawn_sse_mcp_server() -> String {
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    listener.set_nonblocking(true).expect("nonblocking");
    let listener = TcpListener::from_std(listener).expect("from_std");

    // The GET socket, shared so POST handlers can push responses onto it.
    let stream: Arc<AsyncMutex<Option<tokio::net::TcpStream>>> = Arc::new(AsyncMutex::new(None));
    let listed_once = Arc::new(std::sync::atomic::AtomicBool::new(false));

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let stream = Arc::clone(&stream);
            let listed_once = Arc::clone(&listed_once);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..n]).to_string();

                if text.starts_with("GET") {
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
                              event: endpoint\ndata: /messages\n\n",
                        )
                        .await;
                    let _ = socket.flush().await;
                    *stream.lock().await = Some(socket);
                    // Hold the task; the socket now lives in the shared slot.
                    std::future::pending::<()>().await;
                    return;
                }

                let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                let _ = socket
                    .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = socket.flush().await;

                let id = body
                    .split("\"id\":")
                    .nth(1)
                    .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()))
                    .unwrap_or("1")
                    .to_string();

                let mut frames: Vec<String> = Vec::new();
                if body.contains("\"initialize\"") {
                    frames.push(format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{"listChanged":true}}}}}}}}"#
                    ));
                } else if body.contains("\"tools/list\"") {
                    let first = !listed_once.swap(true, std::sync::atomic::Ordering::SeqCst);
                    let tools = if first {
                        r#"[{"name":"alpha","description":"d","inputSchema":{"type":"object"}}]"#
                    } else {
                        r#"[{"name":"alpha","description":"d","inputSchema":{"type":"object"}},{"name":"beta","description":"d","inputSchema":{"type":"object"}}]"#
                    };
                    frames.push(format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":{{"tools":{tools}}}}}"#
                    ));
                    if first {
                        // The mid-session registration announcement.
                        frames.push(
                            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#
                                .to_string(),
                        );
                    }
                }

                if frames.is_empty() {
                    return;
                }
                let mut slot = stream.lock().await;
                if let Some(sse) = slot.as_mut() {
                    for frame in frames {
                        let _ = sse
                            .write_all(format!("event: message\ndata: {frame}\n\n").as_bytes())
                            .await;
                    }
                    let _ = sse.flush().await;
                }
            });
        }
    });

    format!("http://{addr}/sse")
}

/// THE TICKET, end to end. A tool registered by an SSE MCP server AFTER connect
/// must become visible to the manager — the property #1175 states and the one
/// that decides whether the tool is callable at all.
#[tokio::test]
async fn the_manager_picks_up_a_tool_an_sse_server_registers_mid_session() {
    use std::collections::HashMap as Map;

    use wcore_config::config::{McpServerConfig, TransportType};
    use wcore_mcp::manager::McpManager;

    let url = spawn_sse_mcp_server();
    let mut configs = Map::new();
    configs.insert(
        "hosted".to_string(),
        McpServerConfig {
            transport: TransportType::Sse,
            command: None,
            args: None,
            env: None,
            url: Some(url),
            headers: None,
            deferred: None,
            allow_local: true,
            only_for_assistant: None,
            allowed_tools: None,
        },
    );

    let manager = McpManager::connect_all(&configs)
        .await
        .expect("the SSE MCP server must connect");
    let before: Vec<String> = manager
        .all_tools()
        .into_iter()
        .map(|(_, tool)| tool.name)
        .collect();
    assert_eq!(
        before,
        vec!["alpha".to_string()],
        "precondition: only the connect-time tool is advertised"
    );

    // The engine calls this at the top of every turn.
    let mut refreshed = Vec::new();
    for _ in 0..100 {
        refreshed = manager.refresh_signalled_tools().await;
        if !refreshed.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        refreshed,
        vec!["hosted".to_string()],
        "the server announced tools/list_changed; the manager must re-list it"
    );
    let after: Vec<String> = manager
        .all_tools()
        .into_iter()
        .map(|(_, tool)| tool.name)
        .collect();
    assert_eq!(
        after,
        vec!["alpha".to_string(), "beta".to_string()],
        "the mid-session tool must now be advertised, or it stays uncallable \
         for the life of the session"
    );
}

/// The resurrection hazard named alongside the defect. With
/// `take_tools_changed` now implemented for Streamable-HTTP, a transport that
/// reported itself alive forever would let the manager re-list — and
/// re-register — the tools of a server the operator had already removed, on
/// that server's next `list_changed`. `refresh_signalled_tools` gates on
/// `is_alive()`, so closing the transport has to be observable there.
///
/// The OPEN arm is the control and runs first: it proves the refresh really
/// does fire for this server, so the closed arm's empty result cannot be an
/// artefact of nothing ever being signalled.
#[tokio::test]
async fn a_closed_streamable_http_server_is_never_refreshed_again() {
    use wcore_mcp::manager::McpManager;

    let url = spawn_http_server(
        concat!(
            "data: ",
            r#"{"jsonrpc":"2.0","id":10,"result":{"tools":[{"name":"alpha","description":"d","inputSchema":{"type":"object"}}]}}"#,
            "\n\n"
        ),
        true,
        concat!(
            "data: ",
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
            "\n\n"
        ),
    );
    let transport = StreamableHttpTransport::connect(&url, &HashMap::new(), true)
        .await
        .expect("streamable-http connect");
    transport.start_notification_stream().await;

    let manager = McpManager::new_for_test(vec![("removed", false, Box::new(transport))]);

    // CONTROL — the server is announcing and the manager must act on it.
    let mut refreshed = Vec::new();
    for _ in 0..100 {
        refreshed = manager.refresh_signalled_tools().await;
        if !refreshed.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        refreshed,
        vec!["removed".to_string()],
        "precondition: an OPEN server that announces list_changed must be re-listed"
    );

    manager
        .close_server("removed")
        .await
        .expect("close_server must succeed");

    // The server goes on announcing regardless; the manager must stop caring.
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(
            manager.refresh_signalled_tools().await.is_empty(),
            "a server whose transport was closed must never be re-listed, \
             however loudly it goes on announcing tools/list_changed"
        );
    }
}
