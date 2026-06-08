//! v0.6.4 Task 2.4: `mcp-serve` subcommand — exposes the engine's
//! `ToolRegistry` as a real MCP server (stdio or SSE).
//!
//! This module owns two things:
//!   1. `tool_registry_to_server_specs` — adapter from `ToolRegistry`
//!      (engine-side: `Box<dyn Tool>`) to `Vec<ServerToolSpec>` (MCP-side:
//!      JSON-RPC `tools/list` payload entries). The default
//!      `default_tool_set()` in `wcore-mcp` returns empty by design; this
//!      adapter is what actually populates the server.
//!   2. `run` — the subcommand entry point invoked from `main.rs`. Parses
//!      transport selection, builds an `McpServer`, and hands it to
//!      `serve_stdio` / `serve_sse`.
//!
//! Policy gating: as of v0.6.4 Task 2.5 this module uses a real
//! [`PolicyGateAdapter`] wrapping a workspace
//! [`wcore_agent::policy_gate::PolicyGate`]. The caller (currently the
//! `mcp-serve` subcommand in `main.rs`) constructs the gate with grants
//! covering exactly the tools it wants to expose over the wire — the
//! adapter then denies any `tools/call` that lacks a matching grant with
//! `error_code::POLICY_DENIED`.

use std::net::SocketAddr;

use clap::Args;

use wcore_agent::policy_gate::PolicyGate;
use wcore_mcp::{McpServer, ServerToolSpec, SseConfig, serve_sse, serve_stdio};
use wcore_tools::registry::ToolRegistry;

use crate::policy_gate_adapter::PolicyGateAdapter;

/// Convert a `ToolRegistry` into the MCP server's `Vec<ServerToolSpec>`.
///
/// Uses `ToolRegistry::to_tool_defs` (already-resolved name/description/
/// input schema for every registered tool) so we don't have to reach
/// through `&dyn Tool` here.
///
/// Deferred tools (those that `is_deferred()`) are still surfaced —
/// external MCP clients want the full schema; `deferred` is an
/// engine-side LLM-prompt-budget optimization that isn't meaningful
/// over the MCP wire.
pub fn tool_registry_to_server_specs(registry: &ToolRegistry) -> Vec<ServerToolSpec> {
    registry
        .to_tool_defs()
        .into_iter()
        .map(|def| ServerToolSpec {
            name: def.name,
            description: def.description,
            schema_json: def.input_schema,
        })
        .collect()
}

/// CLI args for `wayland-core mcp-serve`.
#[derive(Debug, Args)]
pub struct McpServeArgs {
    /// Transport: `stdio` (default — Claude Desktop / MCP CLIs spawn the
    /// process and speak newline-delimited JSON-RPC over stdin/stdout) or
    /// `sse` (HTTP server with text/event-stream responses on `--bind`).
    #[arg(long, default_value = "stdio", value_parser = ["stdio", "sse"])]
    pub transport: String,

    /// Bind address for `--transport sse`. Ignored for stdio. Defaults to
    /// `127.0.0.1:9876` (loopback only — matches `SseConfig::default`).
    #[arg(long, default_value = "127.0.0.1:9876")]
    pub bind: SocketAddr,
}

/// Build the `McpServer` from a populated `ToolRegistry` + a configured
/// `PolicyGate`. Separate from `run` so an embedder (or a test) can
/// construct the server without touching transports.
///
/// The `PolicyGate` is **required**, not optional: shipping an MCP server
/// over a network transport (sse) or even stdio against an unknown client
/// without a policy gate would mean any caller can invoke any registered
/// tool. v0.6.4 Task 2.5 closes that gap. Callers that want an
/// allow-everything posture (tests, trusted local pipelines) can build a
/// `PolicyGate` over a `PolicyEngine` with broad grants — or use
/// `wcore_mcp::AllowAll` directly via `McpServer::new`.
pub fn build_server(registry: &ToolRegistry, gate: PolicyGate) -> McpServer {
    let specs = tool_registry_to_server_specs(registry);
    let adapter = PolicyGateAdapter::new(gate);
    McpServer::new(specs, Box::new(adapter))
}

/// Subcommand entry point. Builds the server, then drives the requested
/// transport until EOF (stdio) or until externally aborted (sse).
pub async fn run(
    args: McpServeArgs,
    registry: ToolRegistry,
    gate: PolicyGate,
) -> anyhow::Result<()> {
    let server = build_server(&registry, gate);
    match args.transport.as_str() {
        "stdio" => serve_stdio(server).await?,
        "sse" => {
            let cfg = SseConfig { bind: args.bind };
            serve_sse(server, cfg).await?;
        }
        // Unreachable: clap's `value_parser = ["stdio", "sse"]` rejects
        // anything else before we get here.
        other => anyhow::bail!("unknown --transport: {other}"),
    }
    Ok(())
}
