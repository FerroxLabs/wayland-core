//! v0.6.5 Task 3.4 — MCP-bridge end-to-end JSON-RPC drills.
//!
//! Drives `McpBridgePluginRunner::load_with_transport` against a fake MCP
//! server running on the other half of a `tokio::io::duplex` pair. The
//! fixture honors:
//!
//! - `initialize` (request, id=N) → result envelope
//! - `notifications/initialized` (notification, no id) → drop
//! - `tools/list` (request) → ToolsListResult envelope
//! - `tools/call` (request) → McpToolResult envelope with text content
//!
//! Covers full lifecycle, PluginTool synthesis count, and broken-pipe
//! error propagation per the task brief.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};
use wcore_plugin_api::access_gate::PluginAccessGate;
use wcore_plugin_api::tool::{PluginToolCaps, PluginToolEmit, PluginToolInvocation};
use wcore_plugin_subprocess::error::SubprocessPluginError;
use wcore_plugin_subprocess::mcp_bridge::McpBridgePluginRunner;
use wcore_protocol::events::ToolCategory;

/// Read one JSON-RPC line from the host side.
async fn read_line<R>(reader: &mut R) -> Option<Value>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await.ok()?;
    if n == 0 {
        return None;
    }
    serde_json::from_str(line.trim_end()).ok()
}

async fn write_response<W>(writer: &mut W, body: Value)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(&body).expect("serialize response");
    line.push('\n');
    writer.write_all(line.as_bytes()).await.expect("write line");
    writer.flush().await.expect("flush");
}

/// Helper — build a JSON-RPC success response envelope for the given id.
fn jsonrpc_ok(id: u64, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

#[tokio::test]
async fn mcp_handshake_then_list_tools_then_call_tool() {
    // Plugin-side reads requests from `host_to_plugin_r` and writes
    // responses to `plugin_to_host_w`.
    let (host_to_plugin_w, host_to_plugin_r) = duplex(8192);
    let (plugin_to_host_w, plugin_to_host_r) = duplex(8192);

    let fixture = tokio::spawn(async move {
        let mut reader = BufReader::new(host_to_plugin_r);
        let mut writer = plugin_to_host_w;

        // 1. initialize
        let req = read_line(&mut reader).await.expect("initialize request");
        assert_eq!(req["method"], "initialize");
        let init_id = req["id"].as_u64().expect("initialize id");
        write_response(
            &mut writer,
            jsonrpc_ok(
                init_id,
                json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-mcp", "version": "0.1.0"}
                }),
            ),
        )
        .await;

        // 2. notifications/initialized — fire-and-forget, no id, no reply.
        let notif = read_line(&mut reader).await.expect("initialized notif");
        assert_eq!(notif["method"], "notifications/initialized");
        assert!(notif.get("id").is_none(), "notifications must have no id");

        // 3. tools/list — return 3 tools.
        let req = read_line(&mut reader).await.expect("tools/list request");
        assert_eq!(req["method"], "tools/list");
        let list_id = req["id"].as_u64().expect("tools/list id");
        write_response(
            &mut writer,
            jsonrpc_ok(
                list_id,
                json!({
                    "tools": [
                        {"name": "echo", "description": "Echoes text",
                         "inputSchema": {"type": "object",
                                          "properties": {"text": {"type": "string"}}}},
                        {"name": "add",  "description": "Adds two numbers",
                         "inputSchema": {"type": "object"}},
                        {"name": "ping", "description": null,
                         "inputSchema": {"type": "object"}},
                    ]
                }),
            ),
        )
        .await;

        // 4. tools/call for `echo` → text content.
        let req = read_line(&mut reader).await.expect("tools/call request");
        assert_eq!(req["method"], "tools/call");
        assert_eq!(req["params"]["name"], "echo");
        assert_eq!(req["params"]["arguments"], json!({"text": "hello"}));
        let call_id = req["id"].as_u64().expect("tools/call id");
        write_response(
            &mut writer,
            jsonrpc_ok(
                call_id,
                json!({
                    "content": [
                        {"type": "text", "text": "hello"}
                    ],
                    "isError": false,
                }),
            ),
        )
        .await;

        // Drop writer → EOF on host's reader; runner reaper exits cleanly.
        drop(writer);
    });

    let gate = Arc::new(PluginAccessGate);
    let loaded =
        McpBridgePluginRunner::load_with_transport(host_to_plugin_w, plugin_to_host_r, gate)
            .await
            .expect("load mcp bridge");

    // Synthesized PluginTool count matches the fake server's tools/list.
    assert_eq!(loaded.tool_count(), 3);
    let tool_names: Vec<&str> = loaded.tools().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tool_names, vec!["echo", "add", "ping"]);

    // Every synthesized tool is categorized as ToolCategory::Mcp.
    for t in loaded.tools() {
        assert_eq!(
            t.category,
            ToolCategory::Mcp,
            "tool {} should be Mcp",
            t.name
        );
        assert!(!t.is_deferred);
    }

    // Synthesized closures forward through call_mcp_tool.
    let echo_tool = loaded
        .tools()
        .iter()
        .find(|t| t.name == "echo")
        .expect("echo tool")
        .clone();
    let inv = PluginToolInvocation {
        input: json!({"text": "hello"}),
        emit: PluginToolEmit::new(Arc::new(|_| {}), Arc::new(|_, _| {})),
        caps: PluginToolCaps::v1(tokio_util::sync::CancellationToken::new(), "c1", None),
    };
    let result = (echo_tool.execute)(inv).await;
    assert!(!result.is_error, "echo should not error: {result:?}");
    assert_eq!(result.content, "hello");

    let _ = loaded.runner().shutdown().await;
    fixture.await.expect("fixture join");
}

#[tokio::test]
async fn mcp_tool_appears_in_synthesized_initialize_outcome_surface() {
    // The host loader (Task 2.7) will fold `loaded.tools()` directly into
    // `InitializeOutcome.tools`. We can't construct an `InitializeOutcome`
    // from this crate (forbidden upstream dep), but we can prove the
    // surface our loader exposes IS exactly the `PluginTool` collection
    // that apply.rs consumes elsewhere in the engine — same Vec<PluginTool>
    // shape, same metadata fidelity, same execute-closure contract.
    let (host_to_plugin_w, host_to_plugin_r) = duplex(8192);
    let (plugin_to_host_w, plugin_to_host_r) = duplex(8192);

    let fixture = tokio::spawn(async move {
        let mut reader = BufReader::new(host_to_plugin_r);
        let mut writer = plugin_to_host_w;

        // initialize → ok
        let req = read_line(&mut reader).await.unwrap();
        let id = req["id"].as_u64().unwrap();
        write_response(
            &mut writer,
            jsonrpc_ok(
                id,
                json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                }),
            ),
        )
        .await;

        // initialized notif
        let _ = read_line(&mut reader).await.unwrap();

        // tools/list → 1 tool with a rich input schema we want to verify
        // round-trips intact into the synthesized PluginTool.
        let req = read_line(&mut reader).await.unwrap();
        let id = req["id"].as_u64().unwrap();
        write_response(
            &mut writer,
            jsonrpc_ok(
                id,
                json!({
                    "tools": [{
                        "name": "search",
                        "description": "Searches the corpus",
                        "inputSchema": {
                            "type": "object",
                            "required": ["query"],
                            "properties": {
                                "query": {"type": "string", "minLength": 1},
                                "limit": {"type": "integer", "default": 10}
                            }
                        }
                    }]
                }),
            ),
        )
        .await;

        drop(writer);
    });

    let gate = Arc::new(PluginAccessGate);
    let loaded =
        McpBridgePluginRunner::load_with_transport(host_to_plugin_w, plugin_to_host_r, gate)
            .await
            .expect("load");

    assert_eq!(loaded.tool_count(), 1);
    let t = &loaded.tools()[0];
    assert_eq!(t.name, "search");
    assert_eq!(t.description, "Searches the corpus");
    assert_eq!(t.category, ToolCategory::Mcp);

    // The MCP input schema rode through unchanged — the host's apply
    // pipeline will surface the same JSON Schema to the LLM.
    assert_eq!(t.input_schema["required"][0], "query");
    assert_eq!(t.input_schema["properties"]["limit"]["default"], 10);
    assert_eq!(t.input_schema["properties"]["query"]["minLength"], 1);

    fixture.await.expect("fixture join");
}

#[tokio::test]
async fn mcp_server_crash_yields_typed_error() {
    // MCP server completes init + tools/list, then drops both pipes mid-call.
    let (host_to_plugin_w, host_to_plugin_r) = duplex(8192);
    let (plugin_to_host_w, plugin_to_host_r) = duplex(8192);

    let fixture = tokio::spawn(async move {
        let mut reader = BufReader::new(host_to_plugin_r);
        let mut writer = plugin_to_host_w;

        // initialize
        let req = read_line(&mut reader).await.unwrap();
        let id = req["id"].as_u64().unwrap();
        write_response(
            &mut writer,
            jsonrpc_ok(
                id,
                json!({"protocolVersion": "2025-03-26", "capabilities": {}}),
            ),
        )
        .await;
        // initialized notif
        let _ = read_line(&mut reader).await.unwrap();
        // tools/list
        let req = read_line(&mut reader).await.unwrap();
        let id = req["id"].as_u64().unwrap();
        write_response(
            &mut writer,
            jsonrpc_ok(
                id,
                json!({"tools": [{"name": "doomed", "description": "x",
                                  "inputSchema": {"type": "object"}}]}),
            ),
        )
        .await;

        // Read the tools/call request, then drop writer (no reply).
        let _ = read_line(&mut reader).await;
        drop(writer);
    });

    let gate = Arc::new(PluginAccessGate);
    let loaded =
        McpBridgePluginRunner::load_with_transport(host_to_plugin_w, plugin_to_host_r, gate)
            .await
            .expect("load");

    assert_eq!(loaded.tool_count(), 1);

    // Drive the synthesized tool's execute closure directly. The MCP
    // server has dropped its writer, so reader_task drains pending
    // senders → WorkerTerminated → wrapped into ToolResult.is_error.
    let doomed = loaded
        .tools()
        .iter()
        .find(|t| t.name == "doomed")
        .unwrap()
        .clone();
    let inv = PluginToolInvocation {
        input: json!({}),
        emit: PluginToolEmit::new(Arc::new(|_| {}), Arc::new(|_, _| {})),
        caps: PluginToolCaps::v1(tokio_util::sync::CancellationToken::new(), "c1", None),
    };
    let result = tokio::time::timeout(Duration::from_secs(2), (doomed.execute)(inv))
        .await
        .expect("execute should not hang");
    assert!(result.is_error, "expected error result, got {result:?}");
    assert!(
        result.content.contains("doomed"),
        "error message should mention tool name: {}",
        result.content
    );

    // Direct call on the runner also surfaces the typed transport error.
    let direct = loaded.runner().call_mcp_tool("doomed", json!({})).await;
    assert!(
        matches!(
            direct,
            Err(SubprocessPluginError::WorkerTerminated)
                | Err(SubprocessPluginError::Timeout)
                | Err(SubprocessPluginError::BrokenPipe)
        ),
        "expected transport-class error, got {direct:?}"
    );

    fixture.await.expect("fixture join");
}
