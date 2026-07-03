//! MCP HTML content round-trip tests.
//!
//! Verifies that `McpManager` correctly handles tool results that contain
//! HTML-shaped text content. A `QueueTransport` (in-process mock) returns a
//! canned MCP `tools/call` response; the test asserts that the text content
//! flows back through `call_tool` unchanged.
//!
//! This is NOT a test of MCP→browser integration. Real browser interop
//! (spawning a browser process via MCP) is a follow-up integration test.

#![cfg(feature = "test-utils")]

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_mcp::manager::{McpManager, TestServerEntry};
use wcore_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef};
use wcore_mcp::transport::{McpError, McpTransport};

// ---------------------------------------------------------------------------
// Mock transport — returns canned MCP tool results (in-process, no browser).
// ---------------------------------------------------------------------------

/// Canned response queue: returns responses in order, one per request call.
struct QueueTransport {
    responses: Mutex<Vec<Value>>,
}

impl QueueTransport {
    fn with_tool_result(text: &str) -> Self {
        // MCP `tools/call` result shape: {content: [{type: "text", text: "..."}]}
        let response = json!({
            "content": [
                {"type": "text", "text": text}
            ]
        });
        Self {
            responses: Mutex::new(vec![response]),
        }
    }
}

#[async_trait]
impl McpTransport for QueueTransport {
    async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let value = self
            .responses
            .lock()
            .unwrap()
            .drain(0..1)
            .next()
            .unwrap_or(json!(null));
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(value),
            error: None,
        })
    }

    async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
        Ok(())
    }

    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_tool_html_content_flows_back_through_call_tool() {
    // HTML-shaped text that the mock MCP server returns as its tool result.
    let browser_page_content =
        "<title>Example Domain</title><p>This domain is for use in illustrative examples.</p>";

    // Build an McpManager with one pre-wired server that exposes `fetch_page`.
    let tool_def = McpToolDef {
        name: "fetch_page".to_string(),
        description: Some("Fetch a URL via the browser tool and return rendered text".to_string()),
        input_schema: json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
    };

    let entries: Vec<TestServerEntry> = vec![(
        "html-content-mcp",
        false,
        Box::new(QueueTransport::with_tool_result(browser_page_content)),
        vec![tool_def],
    )];

    let manager = McpManager::new_for_test_with_tools(entries);

    // Verify tool is discoverable.
    assert!(
        manager.has_tool_name("fetch_page"),
        "fetch_page must be discoverable via has_tool_name"
    );

    // Call the tool — the mock transport returns canned HTML content.
    let result = manager
        .call_tool(
            "html-content-mcp",
            "fetch_page",
            json!({"url": "https://example.com"}),
        )
        .await
        .expect("call_tool must succeed");

    // HTML content flows back through McpManager unchanged.
    assert!(
        result.text.contains("Example Domain"),
        "HTML title must be present in result; got: {result:?}"
    );
    assert!(
        result.text.contains("illustrative examples"),
        "HTML body must be present in result; got: {result:?}"
    );
}

#[tokio::test]
async fn agent_mcp_tool_server_not_found_returns_error() {
    let manager = McpManager::new_for_test_with_tools(vec![]);

    let err = manager
        .call_tool("nonexistent-server", "any_tool", json!({}))
        .await
        .expect_err("missing server must return error");

    assert!(
        matches!(err, McpError::ServerNotFound(_)),
        "expected ServerNotFound, got {err:?}"
    );
}
