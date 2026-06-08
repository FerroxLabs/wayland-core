//! Declarative MCP server registration. The host adapter calls into
//! `wcore_mcp::tool_proxy::register_mcp_tools` (verified at
//! `crates/wcore-mcp/src/tool_proxy.rs:114`) after `initialize()` returns.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpServerSpec {
    pub name: String,
    pub transport: McpTransport,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Sse { url: String },
    Http { url: String },
}
