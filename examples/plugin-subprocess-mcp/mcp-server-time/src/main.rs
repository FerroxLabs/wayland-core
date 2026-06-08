//! Tiny demo MCP server. Speaks Model Context Protocol JSON-RPC 2.0 over
//! stdio (newline-delimited frames). Exposes a single tool `get_time`.
//!
//! Wayland's `wcore-plugin-subprocess::mcp_bridge::McpBridgePluginRunner`
//! wraps any conformant MCP server like this one and auto-synthesizes
//! Wayland plugin tools from its `tools/list` response — no per-server
//! adapter code required.
//!
//! Protocol surface implemented:
//!   - `initialize`               -> server info + protocol version
//!   - `notifications/initialized`-> drop (notification, no reply)
//!   - `tools/list`               -> one tool: `get_time`
//!   - `tools/call`               -> dispatch to `get_time`
//!
//! Anything else returns JSON-RPC error -32601 (Method not found).
//!
//! Time semantics: returns seconds-since-UNIX-epoch as a UTC string. The
//! `timezone` argument is echoed back as a label so users can see it round-
//! tripped through the bridge; this demo intentionally does not pull in
//! `chrono-tz` to keep build time tiny. A production wrapper would.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF — host closed stdin, exit cleanly.
            Ok(_) => {}
            Err(_) => break,
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                // Unparseable line — JSON-RPC says we can't even respond
                // (no id). Drop and continue.
                continue;
            }
        };

        // Notifications have no `id`; we never reply.
        if req.get("id").is_none() {
            // Only one notification we care about — initialized. Anything
            // else: ignore.
            continue;
        }
        let id = req["id"].clone();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "mcp-server-time",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "get_time",
                            "description": "Returns the current time. The `timezone` argument is echoed as a label; this demo always reports UTC seconds-since-epoch.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "timezone": {
                                        "type": "string",
                                        "description": "Display label only (e.g. \"UTC\", \"America/New_York\")."
                                    }
                                }
                            }
                        }
                    ]
                }
            }),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                match name {
                    "get_time" => call_get_time(id, &args),
                    other => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32602,
                            "message": format!("Unknown tool: {other}")
                        }
                    }),
                }
            }
            other => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {other}")
                }
            }),
        };

        let mut out = serde_json::to_string(&response).expect("serialize response");
        out.push('\n');
        if stdout.write_all(out.as_bytes()).await.is_err() {
            break;
        }
        if stdout.flush().await.is_err() {
            break;
        }
    }
}

fn call_get_time(id: Value, args: &Value) -> Value {
    let tz_label = args
        .get("timezone")
        .and_then(|v| v.as_str())
        .unwrap_or("UTC");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let text = format!("{} (timezone label: {})", now, tz_label);
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [
                { "type": "text", "text": text }
            ],
            "isError": false
        }
    })
}
