//! Wave RA RELIABILITY BLOCKER #2 — verify that an MCP tool call
//! cancelled mid-flight returns promptly. The failure mode without the
//! fix: `McpToolProxy::execute` awaits the JSON-RPC `request` future,
//! which can stall for the MCP server's full 30s+ default timeout
//! before failing. The cancel race in `execute_with_ctx` (added in W8a
//! A.4) wins against that delay; this test pins the contract.
//!
//! Wave RA additionally adds `pool_idle_timeout(5s)` to the reqwest
//! clients in the HTTP/SSE transports so a cancelled-mid-flight request
//! doesn't leave the underlying TCP connection loitering in the pool.
//! Verifying the connection-pool side requires a real HTTP server; the
//! cancel-race side is verified here through a slow `McpTransport` mock.

#![cfg(feature = "test-utils")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use wcore_mcp::manager::McpManager;
use wcore_mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
use wcore_mcp::tool_proxy::McpToolProxy;
use wcore_mcp::transport::{McpError, McpTransport};
use wcore_tools::NullToolOutputSink;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::vfs::RealFs;

/// Transport that simulates a 30s server delay — stand-in for an MCP
/// server that hangs without responding. The `McpToolProxy` cancel race
/// must beat this delay.
struct SlowTransport;

#[async_trait]
impl McpTransport for SlowTransport {
    async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        tokio::time::sleep(Duration::from_secs(30)).await;
        // We never reach here in the cancel test; if we do, return a
        // shape that wouldn't satisfy the assertions below either way.
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(json!({"content": [{"type": "text", "text": "late"}]})),
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

fn build_proxy() -> McpToolProxy {
    let manager = Arc::new(McpManager::new_for_test(vec![(
        "slow-server",
        false,
        Box::new(SlowTransport),
    )]));
    McpToolProxy::new(
        "slow_tool".into(),
        "slow_tool".into(),
        "slow-server".into(),
        "A slow MCP tool for cancel testing".into(),
        json!({"type": "object"}),
        manager,
        false,
    )
}

#[tokio::test]
async fn mcp_tool_returns_promptly_when_cancelled_pre_fire() {
    let proxy = build_proxy();
    let cancel = CancellationToken::new();
    cancel.cancel(); // pre-fire
    let ctx = ToolContext::new(
        "ra-mcp-cancel-pre",
        cancel,
        Arc::new(RealFs),
        None,
        Arc::new(NullToolOutputSink),
    );

    let start = Instant::now();
    let result = proxy.execute_with_ctx(json!({}), &ctx).await;
    let elapsed = start.elapsed();

    assert!(result.is_error, "cancelled MCP tool must error");
    let content_lower = result.content.to_lowercase();
    assert!(
        content_lower.contains("cancel") || content_lower.contains("aborted"),
        "expected cancellation message, got: {}",
        result.content
    );
    // 30s slow-transport delay must NOT be observed; the cancel race
    // wins immediately and we return well under 500ms.
    assert!(
        elapsed < Duration::from_millis(500),
        "pre-cancelled MCP tool must return <500ms, took {elapsed:?}"
    );
}

#[tokio::test]
async fn mcp_tool_returns_promptly_when_cancelled_mid_flight() {
    let proxy = build_proxy();
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    // Cancel 200ms after start — well inside the 30s slow-transport sleep.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel2.cancel();
    });
    let ctx = ToolContext::new(
        "ra-mcp-cancel-mid",
        cancel,
        Arc::new(RealFs),
        None,
        Arc::new(NullToolOutputSink),
    );

    let start = Instant::now();
    let result = proxy.execute_with_ctx(json!({}), &ctx).await;
    let elapsed = start.elapsed();

    assert!(result.is_error, "cancelled MCP tool must error");
    let content_lower = result.content.to_lowercase();
    assert!(
        content_lower.contains("cancel") || content_lower.contains("aborted"),
        "expected cancellation message, got: {}",
        result.content
    );
    // 200ms cancel + select-wakeup must land well under 500ms over the
    // 30s slow-transport sleep.
    assert!(
        elapsed < Duration::from_millis(700),
        "mid-flight cancelled MCP tool must return <700ms, took {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(150),
        "must not return BEFORE cancel fired ({elapsed:?})"
    );
}
