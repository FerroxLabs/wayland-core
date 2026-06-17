use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio::time::timeout;
use tracing::warn;

use super::config::{McpServerConfig, TransportType};
use super::protocol::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, JsonRpcRequest,
    McpResource, McpToolDef, McpToolResult, ResourcesListResult, ResourcesReadResult,
    ToolsListResult,
};
use super::transport::sse::SseTransport;
use super::transport::stdio::StdioTransport;
use super::transport::streamable_http::StreamableHttpTransport;
use super::transport::{McpError, McpTransport};

/// Per-server connect budget (audit C2).
///
/// `connect_server` runs the full MCP handshake: spawn transport →
/// `initialize` → `notifications/initialized` → `tools/list`. Each request
/// over a stdio transport is itself bounded (audit C1), but the boot path
/// must not wait the full per-request budget on a server that is wedged
/// before it ever speaks MCP. 30s covers a legitimately slow server (npm
/// cold start, slow network init) while still converting a hung server into
/// a skip so `bootstrap.build()` — and therefore the CLI/TUI — can start.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// A connected MCP server with its discovered tools and capabilities
struct McpServer {
    #[allow(dead_code)]
    name: String,
    transport: Box<dyn McpTransport>,
    tools: Vec<McpToolDef>,
    /// Whether the server declared resources capability in its initialize response
    supports_resources: bool,
}

/// The connect-time outcome for one server, kept on the manager so the cause of
/// a failure survives boot instead of vanishing into a `tracing::warn`.
///
/// Every server name ever *attempted* (config or plugin) gets exactly one entry
/// in [`McpManager::health`], even the ones that never produced a live
/// [`McpServer`]. This is the source of truth for the `/doctor` MCP section and
/// the `McpFailed` protocol event — `servers` stays the source of truth for
/// *live tools*.
#[derive(Debug, Clone)]
pub enum McpServerHealth {
    /// Connected and serving `tool_count` tools.
    Ready { tool_count: usize },
    /// Connect attempt returned a clean error (transport/init/handshake).
    Failed { reason: String },
    /// Connect attempt exceeded the per-server budget before erroring.
    TimedOut { after: Duration },
    /// Registration was skipped by a gate BEFORE any connect was attempted
    /// (e.g. an unreachable transport command). No manager populates this —
    /// it exists so a boot snapshot can carry skipped servers uniformly.
    Skipped { reason: String },
}

/// Internal: the three-way result of one bounded connect attempt. Lets the
/// connect loop record `TimedOut` vs `Failed` distinctly (the boundary between
/// them is lost once flattened to a single `McpError`).
enum ConnectOutcome {
    Ok(Box<McpServer>),
    Failed(String),
    TimedOut(Duration),
}

/// Manages connections to multiple MCP servers
pub struct McpManager {
    servers: HashMap<String, McpServer>,
    /// Per-server connect outcome (every attempted server, including failures).
    health: HashMap<String, McpServerHealth>,
    /// Monotonically increasing request ID counter for all JSON-RPC calls
    next_id: AtomicU64,
}

impl McpManager {
    /// Connect to all configured MCP servers.
    ///
    /// Audit C2 — each per-server connect is bounded by [`CONNECT_TIMEOUT`]
    /// and all connects run concurrently. A server that *hangs* during its
    /// handshake (rather than cleanly erroring) is converted into a skip by
    /// the timeout, restoring the intended "non-fatal, continue with the
    /// other servers" guarantee. Running concurrently means a single slow
    /// server delays only itself, not the whole boot.
    pub async fn connect_all(configs: &HashMap<String, McpServerConfig>) -> Result<Self, McpError> {
        Self::connect_all_with_connect_timeout(configs, CONNECT_TIMEOUT).await
    }

    /// [`connect_all`](Self::connect_all) with an explicit per-server
    /// connect budget. Test seam (audit C2): a short bound lets a test
    /// verify a hung handshake is skipped without waiting the production
    /// 30s budget.
    pub async fn connect_all_with_connect_timeout(
        configs: &HashMap<String, McpServerConfig>,
        connect_timeout: Duration,
    ) -> Result<Self, McpError> {
        let connect_futures = configs.iter().map(|(name, config)| async move {
            let outcome = Self::connect_server_outcome(name, config, connect_timeout).await;
            (name.clone(), outcome)
        });

        let results = futures::future::join_all(connect_futures).await;

        let mut servers = HashMap::new();
        let mut health = HashMap::new();
        for (name, outcome) in results {
            match outcome {
                ConnectOutcome::Ok(server) => {
                    eprintln!(
                        "[mcp] Connected to '{}': {} tools, resources={}",
                        name,
                        server.tools.len(),
                        server.supports_resources,
                    );
                    health.insert(
                        name.clone(),
                        McpServerHealth::Ready {
                            tool_count: server.tools.len(),
                        },
                    );
                    servers.insert(name, *server);
                }
                ConnectOutcome::Failed(reason) => {
                    // Non-fatal: continue with other servers, but keep the
                    // cause so `/doctor` can surface it (was: log-and-forget).
                    tracing::warn!(target: "mcp.manager", server = %name, error = %reason, "failed to connect MCP server");
                    health.insert(name, McpServerHealth::Failed { reason });
                }
                ConnectOutcome::TimedOut(after) => {
                    tracing::warn!(target: "mcp.manager", server = %name, ?after, "MCP server connect timed out");
                    health.insert(name, McpServerHealth::TimedOut { after });
                }
            }
        }

        Ok(Self {
            servers,
            health,
            next_id: AtomicU64::new(10),
        })
    }

    /// `connect_server` wrapped in a connect-budget timeout (audit C2),
    /// returning a three-way [`ConnectOutcome`] so the caller can record
    /// `TimedOut` vs `Failed` distinctly. On elapse the server is skipped
    /// exactly as if it had failed to connect.
    async fn connect_server_outcome(
        name: &str,
        config: &McpServerConfig,
        connect_timeout: Duration,
    ) -> ConnectOutcome {
        match timeout(connect_timeout, Self::connect_server(name, config)).await {
            Ok(Ok(server)) => ConnectOutcome::Ok(Box::new(server)),
            Ok(Err(e)) => ConnectOutcome::Failed(e.to_string()),
            Err(_) => {
                warn!(server = %name, "[mcp] connect timed out — skipping server");
                ConnectOutcome::TimedOut(connect_timeout)
            }
        }
    }

    /// Connect a single additional MCP server after initial setup.
    /// Returns the list of tool names exposed by the server.
    ///
    /// Audit C2 — the handshake is bounded by [`CONNECT_TIMEOUT`] so a
    /// runtime-added server that wedges during its handshake surfaces as a
    /// typed error instead of hanging the caller.
    pub async fn connect_one(
        &mut self,
        name: String,
        config: &McpServerConfig,
    ) -> Result<Vec<String>, McpError> {
        match Self::connect_server_outcome(&name, config, CONNECT_TIMEOUT).await {
            ConnectOutcome::Ok(server) => {
                let tool_names: Vec<String> = server.tools.iter().map(|t| t.name.clone()).collect();
                eprintln!(
                    "[mcp] Connected to '{}': {} tools, resources={}",
                    name,
                    server.tools.len(),
                    server.supports_resources,
                );
                self.health.insert(
                    name.clone(),
                    McpServerHealth::Ready {
                        tool_count: server.tools.len(),
                    },
                );
                self.servers.insert(name, *server);
                Ok(tool_names)
            }
            ConnectOutcome::Failed(reason) => {
                self.health.insert(
                    name,
                    McpServerHealth::Failed {
                        reason: reason.clone(),
                    },
                );
                Err(McpError::Transport(reason))
            }
            ConnectOutcome::TimedOut(after) => {
                self.health
                    .insert(name.clone(), McpServerHealth::TimedOut { after });
                Err(McpError::Transport(format!(
                    "connect to '{name}' timed out after {after:?}"
                )))
            }
        }
    }

    /// Connect to a single MCP server: create transport, initialize, discover tools
    async fn connect_server(name: &str, config: &McpServerConfig) -> Result<McpServer, McpError> {
        let empty_map = HashMap::new();

        // 1. Create transport
        let transport: Box<dyn McpTransport> = match config.transport {
            TransportType::Stdio => {
                let command = config.command.as_deref().ok_or_else(|| {
                    McpError::InitFailed("stdio transport requires 'command'".into())
                })?;
                let args = config.args.as_deref().unwrap_or(&[]);
                let env = config.env.as_ref().unwrap_or(&empty_map);
                Box::new(StdioTransport::spawn(command, args, env).await?)
            }
            TransportType::Sse => {
                let url = config
                    .url
                    .as_deref()
                    .ok_or_else(|| McpError::InitFailed("SSE transport requires 'url'".into()))?;
                let headers = config.headers.as_ref().unwrap_or(&empty_map);
                Box::new(SseTransport::connect(url, headers, config.allow_local).await?)
            }
            TransportType::StreamableHttp => {
                let url = config.url.as_deref().ok_or_else(|| {
                    McpError::InitFailed("streamable-http transport requires 'url'".into())
                })?;
                let headers = config.headers.as_ref().unwrap_or(&empty_map);
                Box::new(StreamableHttpTransport::connect(url, headers, config.allow_local).await?)
            }
        };

        // 2. Initialize handshake
        let init_params = InitializeParams {
            protocol_version: "2025-03-26".to_string(),
            capabilities: ClientCapabilities {
                tools: Some(json!({})),
            },
            client_info: ClientInfo {
                name: "wayland-core".to_string(),
                version: "0.3.0".to_string(),
            },
        };

        let init_req = JsonRpcRequest::new(
            1,
            "initialize",
            Some(serde_json::to_value(&init_params).map_err(|e| {
                McpError::InitFailed(format!("Failed to serialize init params: {}", e))
            })?),
        );

        let init_response = transport.request(&init_req).await?;
        let init_result: InitializeResult = serde_json::from_value(
            init_response
                .result
                .ok_or_else(|| McpError::InitFailed("No result in initialize response".into()))?,
        )
        .map_err(|e| McpError::InitFailed(format!("Failed to parse init result: {}", e)))?;

        // Check whether server declared resources capability
        let supports_resources = init_result
            .capabilities
            .get("resources")
            .map(|v| !v.is_null())
            .unwrap_or(false);

        // 3. Send initialized notification
        let initialized_notification =
            JsonRpcRequest::notification("notifications/initialized", None);
        transport.notify(&initialized_notification).await?;

        // 4. List tools
        let list_req = JsonRpcRequest::new(2, "tools/list", None);
        let list_response = transport.request(&list_req).await?;
        let tools_result: ToolsListResult = serde_json::from_value(
            list_response
                .result
                .ok_or_else(|| McpError::InitFailed("No result in tools/list response".into()))?,
        )
        .map_err(|e| McpError::InitFailed(format!("Failed to parse tools list: {}", e)))?;

        Ok(McpServer {
            name: name.to_string(),
            transport,
            tools: tools_result.tools,
            supports_resources,
        })
    }

    /// Get all discovered tools with their server names.
    ///
    /// Audit C4 — a server whose transport has died mid-session
    /// (`is_alive() == false`) is skipped, so its tools stop being
    /// advertised. Tool registration is one-shot at boot and re-runs only
    /// on the dynamic add path; a server that connects then dies between
    /// boot and a re-registration will not contribute phantom tools the
    /// model can call but never run.
    pub fn all_tools(&self) -> Vec<(&str, &McpToolDef)> {
        let mut result = Vec::new();
        for (server_name, server) in &self.servers {
            if !server.transport.is_alive() {
                continue;
            }
            for tool in &server.tools {
                result.push((server_name.as_str(), tool));
            }
        }
        result
    }

    /// Check if a tool name exists across any *live* server (audit C4).
    pub fn has_tool_name(&self, name: &str) -> bool {
        self.servers
            .values()
            .filter(|s| s.transport.is_alive())
            .any(|s| s.tools.iter().any(|t| t.name == name))
    }

    /// Count how many *live* servers have a tool with the given name.
    pub fn tool_name_count(&self, name: &str) -> usize {
        self.servers
            .values()
            .filter(|s| s.transport.is_alive())
            .filter(|s| s.tools.iter().any(|t| t.name == name))
            .count()
    }

    /// Whether a connected server's transport is still believed live
    /// (audit C4/C7). `false` for an unknown server name.
    pub fn server_is_alive(&self, server_name: &str) -> bool {
        self.servers
            .get(server_name)
            .map(|s| s.transport.is_alive())
            .unwrap_or(false)
    }

    /// Execute a tool on a specific server
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, McpError> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        // Audit C4 — fast-fail on a server known to be dead instead of
        // routing into a transport that will hang or error after a delay.
        if !server.transport.is_alive() {
            return Err(McpError::Transport(format!(
                "MCP server '{}' is no longer running",
                server_name
            )));
        }

        let request = JsonRpcRequest::new(
            self.next_id.fetch_add(1, Ordering::Relaxed),
            "tools/call",
            Some(json!({
                "name": tool_name,
                "arguments": arguments
            })),
        );

        let response = server.transport.request(&request).await?;

        let result_value = response
            .result
            .ok_or_else(|| McpError::Transport("No result in tool call response".into()))?;

        // Parse result and concatenate text content
        let tool_result: McpToolResult = serde_json::from_value(result_value)
            .map_err(|e| McpError::Transport(format!("Failed to parse tool result: {}", e)))?;

        let mut text_parts = Vec::new();
        for content in &tool_result.content {
            match content {
                super::protocol::McpContent::Text { text } => text_parts.push(text.clone()),
                super::protocol::McpContent::Image { mime_type, .. } => {
                    text_parts.push(format!("[image: {}]", mime_type));
                }
                super::protocol::McpContent::Resource { .. } => {
                    text_parts.push("[resource]".to_string());
                }
            }
        }

        Ok(text_parts.join("\n"))
    }

    /// Get names of all connected servers.
    pub fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    /// Per-server connect outcomes (every *attempted* server, including those
    /// that failed or timed out and have no live entry in `servers`). The
    /// source of truth for the `/doctor` MCP section — keyed by server name.
    pub fn health(&self) -> &HashMap<String, McpServerHealth> {
        &self.health
    }

    /// Whether this manager hosts a connected server named `name`. No
    /// allocation (unlike `server_names`, which clones every key) — preferred
    /// for hot per-dispatch lookups.
    pub fn hosts_server(&self, name: &str) -> bool {
        self.servers.contains_key(name)
    }

    /// Check if a connected server declared the resources capability.
    pub fn server_supports_resources(&self, server_name: &str) -> bool {
        self.servers
            .get(server_name)
            .map(|s| s.supports_resources)
            .unwrap_or(false)
    }

    /// List all resources from a server.
    pub async fn list_resources(&self, server_name: &str) -> Result<Vec<McpResource>, McpError> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, "resources/list", None);
        let response = server.transport.request(&request).await?;

        let result_value = response
            .result
            .ok_or_else(|| McpError::Transport("No result in resources/list response".into()))?;

        let list_result: ResourcesListResult = serde_json::from_value(result_value)
            .map_err(|e| McpError::Transport(format!("Failed to parse resources/list: {}", e)))?;

        Ok(list_result.resources)
    }

    /// Read a single resource by URI from a server. Returns the text content.
    pub async fn read_resource(&self, server_name: &str, uri: &str) -> Result<String, McpError> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, "resources/read", Some(json!({ "uri": uri })));
        let response = server.transport.request(&request).await?;

        let result_value = response
            .result
            .ok_or_else(|| McpError::Transport("No result in resources/read response".into()))?;

        let read_result: ResourcesReadResult = serde_json::from_value(result_value)
            .map_err(|e| McpError::Transport(format!("Failed to parse resources/read: {}", e)))?;

        // Return the first text content found
        read_result
            .contents
            .into_iter()
            .find_map(|c| c.text)
            .ok_or_else(|| McpError::Transport(format!("No text content in resource '{}'", uri)))
    }

    /// Gracefully shutdown all servers
    pub async fn shutdown(&self) {
        for (name, server) in &self.servers {
            if let Err(e) = server.transport.close().await {
                eprintln!("[mcp] Error closing '{}': {}", name, e);
            }
        }
    }

    /// Tear down a single server's transport — kills the child and marks
    /// it dead. Audit C7: when an interactive MCP tool call is cancelled,
    /// the wedged child must be killed, otherwise it leaks and (without
    /// id correlation, pre-C3) could desync the next call. After this the
    /// server reports `is_alive() == false`, so `all_tools()` stops
    /// advertising it and `call_tool` fast-fails.
    pub async fn close_server(&self, server_name: &str) {
        if let Some(server) = self.servers.get(server_name)
            && let Err(e) = server.transport.close().await
        {
            warn!(server = %server_name, error = %e, "[mcp] close_server failed");
        }
    }

    /// Test-only constructor: build a manager from pre-configured servers.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_for_test(
        entries: Vec<(&str, bool, Box<dyn super::transport::McpTransport>)>,
    ) -> Self {
        let mut servers = HashMap::new();
        let mut health = HashMap::new();
        for (name, supports_resources, transport) in entries {
            health.insert(name.to_string(), McpServerHealth::Ready { tool_count: 0 });
            servers.insert(
                name.to_string(),
                McpServer {
                    name: name.to_string(),
                    transport,
                    tools: vec![],
                    supports_resources,
                },
            );
        }
        Self {
            servers,
            health,
            next_id: AtomicU64::new(10),
        }
    }

    /// Test-only constructor: build a manager from pre-configured servers
    /// with pre-discovered tools. Used by W6 B.7 to verify boot-time
    /// `McpReady` emission without spinning up real transports.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_for_test_with_tools(entries: Vec<TestServerEntry>) -> Self {
        let mut servers = HashMap::new();
        let mut health = HashMap::new();
        for (name, supports_resources, transport, tools) in entries {
            health.insert(
                name.to_string(),
                McpServerHealth::Ready {
                    tool_count: tools.len(),
                },
            );
            servers.insert(
                name.to_string(),
                McpServer {
                    name: name.to_string(),
                    transport,
                    tools,
                    supports_resources,
                },
            );
        }
        Self {
            servers,
            health,
            next_id: AtomicU64::new(10),
        }
    }
}

/// Test-only entry shape for `McpManager::new_for_test_with_tools`:
/// `(server_name, supports_resources, transport, pre_discovered_tools)`.
#[cfg(any(test, feature = "test-utils"))]
pub type TestServerEntry = (
    &'static str,
    bool,
    Box<dyn super::transport::McpTransport>,
    Vec<McpToolDef>,
);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcResponse;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // MockTransport: returns pre-configured JSON-RPC responses
    // -----------------------------------------------------------------------

    struct MockTransport {
        /// Responses returned in order for each request call
        responses: Mutex<Vec<serde_json::Value>>,
    }

    impl MockTransport {
        fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl McpTransport for MockTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            let mut guard = self.responses.lock().unwrap();
            let value = if guard.is_empty() {
                json!(null)
            } else {
                guard.remove(0)
            };
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

    struct ErrorTransport;

    #[async_trait]
    impl McpTransport for ErrorTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            Err(McpError::Transport("mock transport error".into()))
        }

        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers: build McpManager with pre-configured servers
    // -----------------------------------------------------------------------

    fn make_manager_with_servers(entries: Vec<(&str, bool, Box<dyn McpTransport>)>) -> McpManager {
        McpManager::new_for_test(entries)
    }

    // -----------------------------------------------------------------------
    // TC-2.x: server_supports_resources [black-box + white-box]
    // -----------------------------------------------------------------------

    #[test]
    fn tc_2_1_server_supports_resources_true() {
        // [black-box] TC-2.1: server with resources capability returns true
        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![])),
        )]);

        assert!(manager.server_supports_resources("test-server"));
    }

    #[test]
    fn tc_2_2_server_supports_resources_false() {
        // [black-box] TC-2.2: server without resources capability returns false
        let manager = make_manager_with_servers(vec![(
            "no-resources-server",
            false,
            Box::new(MockTransport::new(vec![])),
        )]);

        assert!(!manager.server_supports_resources("no-resources-server"));
    }

    #[test]
    fn tc_2_3_server_supports_resources_unknown_server() {
        // [black-box] TC-2.3: unknown server name returns false (not error)
        let manager = make_manager_with_servers(vec![]);

        assert!(!manager.server_supports_resources("unknown-server"));
    }

    #[test]
    fn tc_2_wb_supports_resources_from_capabilities_null_value() {
        // [white-box] capabilities.get("resources") = null -> supports_resources = false
        // This is tested via the parsed field; we verify via make_manager helper
        let manager = make_manager_with_servers(vec![(
            "server",
            false, // null resources → false per impl: !v.is_null() = false
            Box::new(MockTransport::new(vec![])),
        )]);

        assert!(!manager.server_supports_resources("server"));
    }

    // -----------------------------------------------------------------------
    // TC-2.10/2.11: server_names [black-box]
    // -----------------------------------------------------------------------

    #[test]
    fn tc_2_10_server_names_returns_all() {
        // [black-box] TC-2.10: server_names returns all connected server names
        let manager = make_manager_with_servers(vec![
            ("server-a", false, Box::new(MockTransport::new(vec![]))),
            ("server-b", true, Box::new(MockTransport::new(vec![]))),
        ]);

        let mut names = manager.server_names();
        names.sort();
        assert_eq!(names, vec!["server-a", "server-b"]);
    }

    #[test]
    fn tc_2_11_server_names_empty_manager() {
        // [black-box] TC-2.11: no connected servers -> empty vec
        let manager = make_manager_with_servers(vec![]);

        assert!(manager.server_names().is_empty());
    }

    #[test]
    fn tc_2_wb_server_names_returns_owned_strings() {
        // [white-box] Decision 1: server_names() returns Vec<String> not Vec<&str>
        let manager = make_manager_with_servers(vec![(
            "my-server",
            false,
            Box::new(MockTransport::new(vec![])),
        )]);

        let names: Vec<String> = manager.server_names();
        assert_eq!(names, vec!["my-server"]);
    }

    // -----------------------------------------------------------------------
    // TC-2.4/2.5: list_resources [black-box]
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tc_2_4_list_resources_normal() {
        // [black-box] TC-2.4: list_resources returns resources from server
        let resources_response = json!({
            "resources": [
                {"uri": "skill://skill-a"},
                {"uri": "skill://skill-b", "name": "Skill B"}
            ]
        });

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![resources_response])),
        )]);

        let result = manager.list_resources("test-server").await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].uri, "skill://skill-a");
        assert_eq!(result[1].uri, "skill://skill-b");
    }

    #[tokio::test]
    async fn tc_2_5_list_resources_empty() {
        // [black-box] TC-2.5: list_resources returns empty list when server has no resources
        let resources_response = json!({"resources": []});

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![resources_response])),
        )]);

        let result = manager.list_resources("test-server").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn tc_2_6_list_resources_server_not_found() {
        // [black-box] TC-2.6: list_resources returns error when server does not exist
        let manager = make_manager_with_servers(vec![]);

        let result = manager.list_resources("nonexistent").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::ServerNotFound(name) => assert_eq!(name, "nonexistent"),
            e => panic!("expected ServerNotFound, got {:?}", e),
        }
    }

    // -----------------------------------------------------------------------
    // TC-2.7/2.8/2.9: read_resource [black-box + white-box]
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tc_2_7_read_resource_returns_text() {
        // [black-box] TC-2.7: read_resource returns text content
        let read_response = json!({
            "contents": [{"uri": "skill://my-skill", "mimeType": "text/plain", "text": "---\ndescription: A skill\n---\n# My Skill\n"}]
        });

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![read_response])),
        )]);

        let result = manager
            .read_resource("test-server", "skill://my-skill")
            .await
            .unwrap();
        assert!(result.contains("description: A skill"));
    }

    #[tokio::test]
    async fn tc_2_8_read_resource_transport_error() {
        // [black-box] TC-2.8: read_resource returns error when server returns transport error
        let manager =
            make_manager_with_servers(vec![("test-server", true, Box::new(ErrorTransport))]);

        let result = manager
            .read_resource("test-server", "skill://nonexistent")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tc_2_9_read_resource_server_not_found() {
        // [black-box] TC-2.9: read_resource returns error when server does not exist
        let manager = make_manager_with_servers(vec![]);

        let result = manager
            .read_resource("nonexistent", "skill://my-skill")
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::ServerNotFound(name) => assert_eq!(name, "nonexistent"),
            e => panic!("expected ServerNotFound, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn tc_2_wb_read_resource_no_text_content_returns_error() {
        // [white-box] Decision 3: find_map returns None when all contents have text=None -> error
        let read_response = json!({
            "contents": [{"uri": "skill://binary", "mimeType": "application/octet-stream"}]
        });

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![read_response])),
        )]);

        let result = manager.read_resource("test-server", "skill://binary").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tc_2_wb_read_resource_find_map_first_text() {
        // [white-box] Decision 3: find_map returns first content with non-None text
        let read_response = json!({
            "contents": [
                {"uri": "skill://x"},
                {"uri": "skill://x", "text": "actual content"}
            ]
        });

        let manager = make_manager_with_servers(vec![(
            "test-server",
            true,
            Box::new(MockTransport::new(vec![read_response])),
        )]);

        let result = manager
            .read_resource("test-server", "skill://x")
            .await
            .unwrap();
        assert_eq!(result, "actual content");
    }

    #[test]
    fn tc_2_wb_next_id_starts_at_10() {
        // [white-box] Decision 4: AtomicU64 counter starts at 10 to avoid conflict with connect_server IDs 1/2
        let manager = make_manager_with_servers(vec![]);
        // next_id is private — we verify by doing two fetch_adds and checking values are 10 and 11
        let id1 = manager
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id2 = manager
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(id1, 10, "first ID should be 10");
        assert_eq!(id2, 11, "second ID should be 11");
    }

    // -----------------------------------------------------------------------
    // Audit C2 — bounded handshake. A server that spawns but never speaks
    // MCP must be skipped (timeout), not hang the whole `connect_all`.
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[tokio::test]
    async fn c2_connect_all_skips_hung_server_and_continues() {
        use wcore_config::config::TransportType;

        // A stdio server that spawns then hangs forever without speaking
        // MCP — `sleep` ignores stdin and never writes a handshake reply.
        let hung = McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("sh".into()),
            args: Some(vec!["-c".into(), "sleep 60".into()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
        };
        let mut configs = HashMap::new();
        configs.insert("hung-server".to_string(), hung);

        let start = std::time::Instant::now();
        // Short connect budget so the test is fast — production uses 30s.
        let manager =
            McpManager::connect_all_with_connect_timeout(&configs, Duration::from_millis(400))
                .await
                .expect("connect_all must succeed (skipping the hung server)");
        let elapsed = start.elapsed();

        // Boot continues: the hung server is skipped, not connected.
        assert!(
            manager.server_names().is_empty(),
            "hung server must be skipped, not registered"
        );
        // The handshake timeout fired — `connect_all` did not hang.
        assert!(
            elapsed < Duration::from_secs(3),
            "connect_all must not hang on a wedged server, took {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn c2_one_hung_server_does_not_block_a_healthy_one() {
        use wcore_config::config::TransportType;

        let hung = McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("sh".into()),
            args: Some(vec!["-c".into(), "sleep 60".into()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
        };
        // A real MCP handshake fixture: answer initialize + tools/list.
        let healthy_script = r#"
            while IFS= read -r line; do
              case "$line" in
                *initialize*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{}}}\n' ;;
                *tools/list*) printf '{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}\n' ;;
              esac
            done
        "#;
        let healthy = McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("sh".into()),
            args: Some(vec!["-c".into(), healthy_script.into()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
        };
        let mut configs = HashMap::new();
        configs.insert("hung".to_string(), hung);
        configs.insert("healthy".to_string(), healthy);

        let manager =
            McpManager::connect_all_with_connect_timeout(&configs, Duration::from_millis(600))
                .await
                .expect("connect_all ok");

        // The healthy server connected; the hung one was skipped.
        let names = manager.server_names();
        assert_eq!(names, vec!["healthy".to_string()], "got {names:?}");
    }

    // -----------------------------------------------------------------------
    // Health capture (Slice A1) — every *attempted* server gets exactly one
    // `health()` entry, with the failure cause preserved (was log-and-forget).
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[tokio::test]
    async fn health_records_timeout_for_a_hung_server() {
        use wcore_config::config::TransportType;

        let hung = McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("sh".into()),
            args: Some(vec!["-c".into(), "sleep 60".into()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
        };
        let mut configs = HashMap::new();
        configs.insert("hung".to_string(), hung);

        let manager =
            McpManager::connect_all_with_connect_timeout(&configs, Duration::from_millis(300))
                .await
                .expect("connect_all ok");

        // No live server, but the timeout is recorded with its cause.
        assert!(manager.server_names().is_empty());
        match manager.health().get("hung") {
            Some(McpServerHealth::TimedOut { after }) => {
                assert_eq!(*after, Duration::from_millis(300));
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn health_records_failure_for_an_unspawnable_server() {
        use wcore_config::config::TransportType;

        // A command that cannot spawn → clean transport error, not a timeout.
        let broken = McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("/nonexistent/wayland-mcp-binary-xyz".into()),
            args: None,
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
        };
        let mut configs = HashMap::new();
        configs.insert("broken".to_string(), broken);

        let manager =
            McpManager::connect_all_with_connect_timeout(&configs, Duration::from_secs(5))
                .await
                .expect("connect_all ok");

        assert!(manager.server_names().is_empty());
        match manager.health().get("broken") {
            Some(McpServerHealth::Failed { reason }) => {
                assert!(!reason.is_empty(), "failure reason must be preserved");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn health_records_ready_with_tool_count_and_covers_every_attempt() {
        use wcore_config::config::TransportType;

        // Healthy fixture advertising exactly one tool.
        let healthy_script = r#"
            while IFS= read -r line; do
              case "$line" in
                *initialize*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{}}}\n' ;;
                *tools/list*) printf '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"e","inputSchema":{"type":"object"}}]}}\n' ;;
              esac
            done
        "#;
        let healthy = McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("sh".into()),
            args: Some(vec!["-c".into(), healthy_script.into()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
        };
        let hung = McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("sh".into()),
            args: Some(vec!["-c".into(), "sleep 60".into()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
        };
        let mut configs = HashMap::new();
        configs.insert("healthy".to_string(), healthy);
        configs.insert("hung".to_string(), hung);

        let manager =
            McpManager::connect_all_with_connect_timeout(&configs, Duration::from_millis(600))
                .await
                .expect("connect_all ok");

        // Every attempted server is in `health` — even the one with no live entry.
        assert_eq!(manager.health().len(), 2, "both attempts recorded");
        match manager.health().get("healthy") {
            Some(McpServerHealth::Ready { tool_count }) => assert_eq!(*tool_count, 1),
            other => panic!("expected Ready{{1}}, got {other:?}"),
        }
        assert!(
            matches!(
                manager.health().get("hung"),
                Some(McpServerHealth::TimedOut { .. })
            ),
            "hung attempt must still be recorded"
        );
    }

    // -----------------------------------------------------------------------
    // Audit C4 — a dead server's tools stop being advertised and `call_tool`
    // fast-fails instead of routing into a hang.
    // -----------------------------------------------------------------------

    /// Mock transport whose `is_alive()` is controllable — stands in for a
    /// server that has died mid-session.
    struct DeadTransport;

    #[async_trait]
    impl McpTransport for DeadTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            Err(McpError::Transport("dead server".into()))
        }
        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
        fn is_alive(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn c4_dead_server_tools_not_advertised() {
        let manager = McpManager::new_for_test_with_tools(vec![
            (
                "live",
                false,
                Box::new(MockTransport::new(vec![])),
                vec![McpToolDef {
                    name: "live_tool".into(),
                    description: None,
                    input_schema: json!({}),
                }],
            ),
            (
                "dead",
                false,
                Box::new(DeadTransport),
                vec![McpToolDef {
                    name: "dead_tool".into(),
                    description: None,
                    input_schema: json!({}),
                }],
            ),
        ]);

        let tools: Vec<&str> = manager
            .all_tools()
            .iter()
            .map(|(_, t)| t.name.as_str())
            .collect();
        assert!(tools.contains(&"live_tool"), "live tool must be advertised");
        assert!(
            !tools.contains(&"dead_tool"),
            "dead server's tool must NOT be advertised (audit C4)"
        );
        assert!(!manager.has_tool_name("dead_tool"));
        assert!(manager.has_tool_name("live_tool"));
        assert!(!manager.server_is_alive("dead"));
        assert!(manager.server_is_alive("live"));
    }

    #[tokio::test]
    async fn c4_call_tool_on_dead_server_fast_fails() {
        let manager = make_manager_with_servers(vec![("dead", false, Box::new(DeadTransport))]);

        let result = manager.call_tool("dead", "anything", json!({})).await;
        assert!(result.is_err(), "call to a dead server must error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("no longer running"),
            "expected a dead-server error, got: {msg}"
        );
    }
}
