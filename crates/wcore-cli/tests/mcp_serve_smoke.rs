//! v0.6.4 Task 2.4: smoke test for the `mcp-serve` subcommand glue.
//!
//! Verifies that:
//!   1. `tool_registry_to_server_specs` produces a non-empty `Vec<ServerToolSpec>`
//!      whose entries mirror the names/descriptions of the registered tools.
//!   2. An `McpServer` constructed from those specs reports the tool through
//!      `tools/list` (proving the adapter is wired through the assembled server,
//!      not just tested in isolation).
//!
//! Drives the server in-process via `McpServer::handle_request` rather than
//! spinning up stdio/SSE — the transports already have full coverage in
//! `wcore-mcp` and we don't want this test to depend on a port or stdin pipe.

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_cli::mcp_serve::tool_registry_to_server_specs;
use wcore_mcp::{AllowAll, McpServer, ServerJsonRpcRequest};
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::tool::{JsonSchema, ToolResult};

/// Minimal stand-in tool — just enough to satisfy `Tool` so it can be
/// registered and surfaced through the adapter. Execute path is never
/// exercised here; this test only inspects the `tools/list` capability
/// advertisement.
struct FakeTool;

#[async_trait]
impl Tool for FakeTool {
    fn name(&self) -> &str {
        "fake_smoke_tool"
    }
    fn description(&self) -> &str {
        "smoke-test tool used to verify mcp-serve adapter wiring"
    }
    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "msg": { "type": "string" }
            },
            "required": ["msg"]
        })
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
    async fn execute(&self, _input: Value) -> ToolResult {
        ToolResult {
            content: "ok".to_string(),
            is_error: false,
        }
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
}

#[tokio::test]
async fn tools_list_includes_registered_fake_tool() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FakeTool));

    let specs = tool_registry_to_server_specs(&registry);
    assert!(
        !specs.is_empty(),
        "adapter must surface registered tools (registry has 1, got {})",
        specs.len()
    );
    assert!(
        specs.iter().any(|s| s.name == "fake_smoke_tool"),
        "adapter output missing fake_smoke_tool: got {:?}",
        specs.iter().map(|s| &s.name).collect::<Vec<_>>()
    );

    let server = McpServer::new(specs, Box::new(AllowAll));
    let req = ServerJsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(1)),
        method: "tools/list".into(),
        params: None,
    };
    let resp = server.handle_request(req).await;
    let result = resp.result.expect("tools/list returns result");
    let tools = result["tools"].as_array().expect("tools is an array");
    assert!(
        !tools.is_empty(),
        "assembled server must advertise the adapter-derived tools (was empty)"
    );
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(
        names.contains(&"fake_smoke_tool"),
        "assembled server tools/list missing fake_smoke_tool: {names:?}"
    );

    // Spot-check the inputSchema round-trips intact through the adapter.
    let entry = tools
        .iter()
        .find(|t| t["name"] == "fake_smoke_tool")
        .expect("entry");
    assert_eq!(entry["inputSchema"]["type"], "object");
    assert_eq!(entry["inputSchema"]["required"][0], "msg");
}
