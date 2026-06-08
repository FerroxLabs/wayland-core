//! T2-E1: MCP server (inverse of the existing MCP client).
//!
//! Hosts the **server side** of the Model Context Protocol — accepting
//! JSON-RPC `initialize`, `tools/list`, and `tools/call` from upstream
//! clients (other agents, IDEs, the AionUI host). Transports live in
//! `crate::transports` (stdio + SSE).
//!
//! ## Why server-side types live here
//!
//! `protocol.rs` defines the **client-side** JSON-RPC envelopes
//! (`JsonRpcRequest: Serialize`, `JsonRpcResponse: Deserialize`) — we
//! send and they reply. The server is the inverse: we deserialize an
//! incoming request and serialize an outgoing response. Rather than
//! retrofit dual-direction derives onto the existing client structs and
//! risk breaking the manager/tool_proxy paths, we mint server-side
//! mirrors in this module. Wire format is identical; only the derive
//! direction flips.
//!
//! ## Policy gating
//!
//! `tools/call` runs `policy.check_tool(name)` before dispatching. The
//! workspace `PolicyGate` lives in `wcore-agent`, which sits **above**
//! `wcore-mcp` in the dep graph — `wcore-mcp` cannot depend on it
//! without a cycle. Instead we expose the `PolicyCheck` trait so the
//! upper layer can plug in an adapter. The default `AllowAll` keeps the
//! server usable in standalone scenarios.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Server-side JSON-RPC request (deserialized from the wire).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerJsonRpcRequest {
    pub jsonrpc: String,
    /// Notifications omit `id`; we preserve `None` and echo `None` back
    /// (callers should not expect a response for notifications, but the
    /// dispatch path stays uniform).
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// Server-side JSON-RPC response (serialized onto the wire).
///
/// `jsonrpc` is `String` rather than `&'static str` so the same struct
/// round-trips through `serde::Deserialize` cleanly — tests parse
/// responses back to compare them, and a `&'static` deserialize target
/// forces `'static` lifetime bounds on borrowed input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerJsonRpcResponse {
    pub jsonrpc: String,
    /// Echo the request id verbatim (string, number, or null per JSON-RPC 2.0).
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ServerJsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerJsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ServerJsonRpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(ServerJsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// JSON-RPC 2.0 standard error codes (subset we use).
pub mod error_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// Implementation-defined: policy denied the call.
    pub const POLICY_DENIED: i64 = -32001;
    /// Implementation-defined: tool not yet implemented (stub).
    pub const NOT_IMPLEMENTED: i64 = -32002;
}

/// One tool advertised by `tools/list`. Stubs for v0.6.2 — the real
/// implementations land in later waves (memory + tool registry wiring).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's input. Emitted under the
    /// `inputSchema` key in `tools/list` (MCP convention).
    pub schema_json: Value,
}

/// Policy gate trait — implemented by the embedding layer. The
/// workspace `PolicyGate` (in `wcore-agent`) cannot be referenced here
/// without a circular dep, so we keep this minimal and let the upper
/// layer adapt.
///
/// v0.6.4 Task 2.5: implemented by `wcore_cli::policy_gate_adapter::PolicyGateAdapter`,
/// which wraps `wcore_agent::policy_gate::PolicyGate` and calls
/// `gate.check_tool(name, None).is_ok()`. The adapter lives one layer up
/// because `wcore-agent → wcore-mcp` would cycle.
pub trait PolicyCheck: Send + Sync {
    fn check_tool(&self, name: &str) -> bool;
}

/// Default policy: permits every tool. Used when the embedder hasn't
/// installed a gate.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAll;

impl PolicyCheck for AllowAll {
    fn check_tool(&self, _name: &str) -> bool {
        true
    }
}

/// Tool-execution trait — implemented by the embedding layer. `wcore-mcp`
/// is a LOW crate and cannot depend on `wcore-tools`/`wcore-agent`, so the
/// real built-in tool registry lives one layer up. The embedder installs a
/// `ServerToolExecutor` whose `call` routes an advertised `tools/call` to
/// the real tool execution path.
///
/// Implemented by `wcore_cli::mcp_serve::RegistryToolExecutor`, which wraps
/// the engine's `ToolRegistry` and dispatches to `Tool::execute_with_ctx`.
///
/// `call` returns the MCP `tools/call` *result* object on success (the
/// `{ "content": [...], "isError": bool }` payload the server emits under
/// the JSON-RPC `result` key). An `Err` is surfaced to the client as an
/// `INTERNAL_ERROR` JSON-RPC error.
#[async_trait]
pub trait ServerToolExecutor: Send + Sync {
    async fn call(&self, name: &str, args: Value) -> anyhow::Result<Value>;
}

/// Bundled tool set advertised by `tools/list`.
///
/// R2 fix A3 (MCP protocol compliance): per the MCP spec, `tools/list` is
/// a *capability* advertisement — clients use it to discover what they can
/// call. Advertising tools that always return `NOT_IMPLEMENTED` violates
/// that contract. Real tool wiring (memory + tool registry) is deferred to
/// v0.6.3+; until then this returns an empty Vec.
///
/// Defense in depth: the four known stub names (`wayland_memory_recall`,
/// `wayland_memory_search`, `wayland_tool_list`, `wayland_tool_describe`)
/// are still recognized by `handle_tools_call` and answered with
/// `NOT_IMPLEMENTED` rather than `METHOD_NOT_FOUND`, so a client that
/// hardcoded those names against an earlier preview build gets the more
/// informative error.
pub fn default_tool_set() -> Vec<ServerToolSpec> {
    Vec::new()
}

/// Stub tool names that are recognized for the NOT_IMPLEMENTED handler
/// path but NOT advertised via `tools/list`. See `default_tool_set` docs.
const KNOWN_STUB_NAMES: &[&str] = &[
    "wayland_memory_recall",
    "wayland_memory_search",
    "wayland_tool_list",
    "wayland_tool_describe",
];

/// MCP server. Construct via `McpServer::new(...)` then drive it from a
/// transport (`crate::transports::serve_stdio` or `serve_sse`).
pub struct McpServer {
    tools: Vec<ServerToolSpec>,
    policy: Box<dyn PolicyCheck>,
    /// Real tool-execution backend, installed by the embedding layer.
    /// `None` keeps the standalone server usable (advertised tools then
    /// fall through to the `NOT_IMPLEMENTED` stub path).
    executor: Option<Arc<dyn ServerToolExecutor>>,
    server_name: String,
    server_version: String,
    protocol_version: String,
}

impl McpServer {
    /// Construct with an explicit tool set + policy.
    pub fn new(tools: Vec<ServerToolSpec>, policy: Box<dyn PolicyCheck>) -> Self {
        Self {
            tools,
            policy,
            executor: None,
            server_name: "wcore-mcp".to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            // The MCP spec evolves; 2024-11-05 is the version both
            // Anthropic's reference impl and our client speak as of
            // v0.6.2. Keep this in one place so it can be bumped
            // centrally.
            protocol_version: "2024-11-05".to_string(),
        }
    }

    /// Install a real tool-execution backend. Advertised tools then
    /// dispatch to it via `tools/call` instead of returning the
    /// `NOT_IMPLEMENTED` stub. Builder-style so call sites read
    /// `McpServer::new(specs, policy).with_executor(exec)`.
    pub fn with_executor(mut self, executor: Arc<dyn ServerToolExecutor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Convenience: default tool set + `AllowAll` policy.
    pub fn with_defaults() -> Self {
        Self::new(default_tool_set(), Box::new(AllowAll))
    }

    /// Read access — used by tests and possibly future introspection.
    pub fn tools(&self) -> &[ServerToolSpec] {
        &self.tools
    }

    /// Dispatch a single JSON-RPC request and return the response.
    /// Notifications (no `id`) still return a response; callers in
    /// notification mode should drop it.
    pub async fn handle_request(&self, req: ServerJsonRpcRequest) -> ServerJsonRpcResponse {
        let id = req.id.clone();
        if req.jsonrpc != "2.0" {
            return ServerJsonRpcResponse::err(
                id,
                error_code::INVALID_REQUEST,
                "jsonrpc must be \"2.0\"",
            );
        }
        match req.method.as_str() {
            "initialize" => self.handle_initialize(id),
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req.params).await,
            other => ServerJsonRpcResponse::err(
                id,
                error_code::METHOD_NOT_FOUND,
                format!("unknown method: {}", other),
            ),
        }
    }

    fn handle_initialize(&self, id: Option<Value>) -> ServerJsonRpcResponse {
        ServerJsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": self.protocol_version,
                "capabilities": {
                    "tools": {"listChanged": false}
                },
                "serverInfo": {
                    "name": self.server_name,
                    "version": self.server_version,
                }
            }),
        )
    }

    fn handle_tools_list(&self, id: Option<Value>) -> ServerJsonRpcResponse {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.schema_json,
                })
            })
            .collect();
        ServerJsonRpcResponse::ok(id, json!({ "tools": tools }))
    }

    async fn handle_tools_call(
        &self,
        id: Option<Value>,
        params: Option<Value>,
    ) -> ServerJsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => {
                return ServerJsonRpcResponse::err(
                    id,
                    error_code::INVALID_PARAMS,
                    "tools/call requires params with `name`",
                );
            }
        };
        let name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => {
                return ServerJsonRpcResponse::err(
                    id,
                    error_code::INVALID_PARAMS,
                    "tools/call params.name missing or non-string",
                );
            }
        };

        // Policy gate first — we deny even unknown tool names by policy
        // so the policy layer can audit attempts.
        if !self.policy.check_tool(&name) {
            return ServerJsonRpcResponse::err(
                id,
                error_code::POLICY_DENIED,
                format!("policy denied tool: {}", name),
            );
        }

        // Lookup
        let advertised = self.tools.iter().any(|t| t.name == name);
        let known_stub = KNOWN_STUB_NAMES.contains(&name.as_str());
        if !advertised && !known_stub {
            return ServerJsonRpcResponse::err(
                id,
                error_code::METHOD_NOT_FOUND,
                format!("unknown tool: {}", name),
            );
        }

        // Real dispatch: if the embedder installed an executor and this is
        // an advertised tool, route the call to it. Per the MCP spec the
        // tool's arguments live under `params.arguments` (object); default
        // to an empty object when omitted.
        if advertised && let Some(executor) = self.executor.as_ref() {
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            return match executor.call(&name, args).await {
                Ok(result) => ServerJsonRpcResponse::ok(id, result),
                Err(e) => ServerJsonRpcResponse::err(
                    id,
                    error_code::INTERNAL_ERROR,
                    format!("tool `{}` execution failed: {:#}", name, e),
                ),
            };
        }

        // No executor wired (standalone server) or a KNOWN_STUB name:
        // answer with NOT_IMPLEMENTED rather than METHOD_NOT_FOUND so a
        // client that hardcoded a stub name gets the more informative
        // error.
        ServerJsonRpcResponse::err(
            id,
            error_code::NOT_IMPLEMENTED,
            format!(
                "tool `{}` has no execution backend wired on this server",
                name
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u64, method: &str, params: Option<Value>) -> ServerJsonRpcRequest {
        ServerJsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(id)),
            method: method.into(),
            params,
        }
    }

    #[tokio::test]
    async fn initialize_returns_protocol_and_server_info() {
        let server = McpServer::with_defaults();
        let resp = server.handle_request(req(1, "initialize", None)).await;
        assert_eq!(resp.id, Some(json!(1)));
        let result = resp.result.expect("result");
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "wcore-mcp");
    }

    /// R2 fix A3: `tools/list` no longer advertises stub tools that
    /// would return NOT_IMPLEMENTED on call (MCP spec compliance —
    /// advertise only what works). Stub names remain reachable via
    /// `tools/call` with NOT_IMPLEMENTED for client backwards-compat.
    #[tokio::test]
    async fn tools_list_returns_empty_when_no_real_tools_wired() {
        let server = McpServer::with_defaults();
        let resp = server.handle_request(req(2, "tools/list", None)).await;
        let result = resp.result.expect("result");
        let tools = result["tools"].as_array().expect("tools array");
        assert_eq!(
            tools.len(),
            0,
            "v0.6.2 advertises no tools — stubs are not advertised, real tools land in v0.6.3+"
        );
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let server = McpServer::with_defaults();
        let resp = server.handle_request(req(3, "bogus", None)).await;
        let err = resp.error.expect("error");
        assert_eq!(err.code, error_code::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn tools_call_known_stub_returns_not_implemented() {
        let server = McpServer::with_defaults();
        let resp = server
            .handle_request(req(
                4,
                "tools/call",
                Some(json!({"name": "wayland_memory_recall"})),
            ))
            .await;
        let err = resp.error.expect("error");
        assert_eq!(err.code, error_code::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn tools_call_unknown_returns_method_not_found() {
        let server = McpServer::with_defaults();
        let resp = server
            .handle_request(req(5, "tools/call", Some(json!({"name": "not_a_tool"}))))
            .await;
        let err = resp.error.expect("error");
        assert_eq!(err.code, error_code::METHOD_NOT_FOUND);
    }

    struct DenyAll;
    impl PolicyCheck for DenyAll {
        fn check_tool(&self, _: &str) -> bool {
            false
        }
    }

    /// End-to-end wiring proof: a server built with a real tool spec + an
    /// installed `ServerToolExecutor` advertises the tool via `tools/list`
    /// AND executes it via `tools/call`, returning the executor's result.
    struct EchoExecutor;
    #[async_trait]
    impl ServerToolExecutor for EchoExecutor {
        async fn call(&self, name: &str, args: Value) -> anyhow::Result<Value> {
            Ok(json!({
                "content": [{ "type": "text", "text": format!("{name}:{args}") }],
                "isError": false,
            }))
        }
    }

    fn echo_spec() -> ServerToolSpec {
        ServerToolSpec {
            name: "echo".into(),
            description: "echoes its input".into(),
            schema_json: json!({ "type": "object" }),
        }
    }

    #[tokio::test]
    async fn tools_list_advertises_real_tools() {
        let server = McpServer::new(vec![echo_spec()], Box::new(AllowAll))
            .with_executor(Arc::new(EchoExecutor));
        let resp = server.handle_request(req(10, "tools/list", None)).await;
        let result = resp.result.expect("result");
        let tools = result["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1, "the real tool is advertised");
        assert_eq!(tools[0]["name"], "echo");
        assert_eq!(tools[0]["inputSchema"], json!({ "type": "object" }));
    }

    #[tokio::test]
    async fn tools_call_dispatches_to_executor() {
        let server = McpServer::new(vec![echo_spec()], Box::new(AllowAll))
            .with_executor(Arc::new(EchoExecutor));
        let resp = server
            .handle_request(req(
                11,
                "tools/call",
                Some(json!({ "name": "echo", "arguments": { "x": 1 } })),
            ))
            .await;
        assert!(
            resp.error.is_none(),
            "expected success, got {:?}",
            resp.error
        );
        let result = resp.result.expect("result");
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().expect("text");
        assert!(text.starts_with("echo:"), "executor ran: {text}");
        assert!(text.contains("\"x\":1"), "args forwarded: {text}");
    }

    #[tokio::test]
    async fn tools_call_executor_error_maps_to_internal_error() {
        struct FailExecutor;
        #[async_trait]
        impl ServerToolExecutor for FailExecutor {
            async fn call(&self, _name: &str, _args: Value) -> anyhow::Result<Value> {
                anyhow::bail!("boom")
            }
        }
        let server = McpServer::new(vec![echo_spec()], Box::new(AllowAll))
            .with_executor(Arc::new(FailExecutor));
        let resp = server
            .handle_request(req(12, "tools/call", Some(json!({ "name": "echo" }))))
            .await;
        let err = resp.error.expect("error");
        assert_eq!(err.code, error_code::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn tools_call_denied_by_policy() {
        let server = McpServer::new(default_tool_set(), Box::new(DenyAll));
        let resp = server
            .handle_request(req(
                6,
                "tools/call",
                Some(json!({"name": "wayland_memory_recall"})),
            ))
            .await;
        let err = resp.error.expect("error");
        assert_eq!(err.code, error_code::POLICY_DENIED);
    }
}
