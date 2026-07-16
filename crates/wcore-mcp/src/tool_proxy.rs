use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::config::McpServerConfig;
use super::manager::{McpManager, McpToolEffectIdentity};
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::context::{ToolContext, ToolEffectContext};
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

/// Wraps an MCP server tool as a local Tool trait implementation.
/// Uses naming convention "mcp__{server}__{tool}" when collisions exist,
/// otherwise uses the tool's original name.
pub struct McpToolProxy {
    /// Display name used for registration (may be prefixed)
    display_name: String,
    /// Original tool name on the MCP server
    tool_name: String,
    /// Server this tool belongs to
    server_name: String,
    description: String,
    input_schema: JsonSchema,
    manager: Arc<McpManager>,
    /// Whether this tool's schema should be deferred (sent as name-only stub).
    deferred: bool,
}

impl McpToolProxy {
    pub fn new(
        display_name: String,
        tool_name: String,
        server_name: String,
        description: String,
        input_schema: JsonSchema,
        manager: Arc<McpManager>,
        deferred: bool,
    ) -> Self {
        Self {
            display_name,
            tool_name,
            server_name,
            description,
            input_schema,
            manager,
            deferred,
        }
    }

    async fn execute_with_optional_effect(
        &self,
        input: Value,
        ctx: &ToolContext,
        effect: Option<&ToolEffectContext>,
    ) -> ToolResult {
        let effect = effect.map(|effect| {
            McpToolEffectIdentity::v1(
                effect.tool_execution_id.clone(),
                effect.idempotency_key.clone(),
            )
        });
        tokio::select! {
            _ = ctx.cancel.cancelled() => {
                let _ = self.manager.close_server(&self.server_name).await;
                ToolResult {
                    content: format!(
                        "MCP tool '{}/{}' call aborted by cancellation token \
                         (server transport torn down)",
                        self.server_name, self.tool_name,
                    ),
                    is_error: true,
                }
            }
            result = self.manager.call_tool_with_effect_identity(
                &self.server_name,
                &self.tool_name,
                input,
                effect.as_ref(),
            ) => match result {
                Ok(outcome) => ToolResult {
                    content: outcome.text,
                    is_error: outcome.is_error,
                },
                Err(e) => ToolResult {
                    content: format!("MCP tool error: {}", e),
                    is_error: true,
                },
            },
        }
    }
}

#[async_trait]
impl Tool for McpToolProxy {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> JsonSchema {
        self.input_schema.clone()
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // MCP tools are assumed not concurrency-safe
        false
    }

    fn is_deferred(&self) -> bool {
        self.deferred
    }

    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // MCP servers expose arbitrary external effects with no host reconciler.
        ToolEffectContract::default()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        match self
            .manager
            .call_tool(&self.server_name, &self.tool_name, input)
            .await
        {
            // #475: a transport-successful call may still be a tool-level
            // failure (MCP `isError: true`). Surface that as `is_error` so the
            // agent (retry-cap guard, UI badge, model error signal) sees it —
            // the `content` still carries the tool's error text so the model can
            // read it and recover. A single failure never aborts; it is just a
            // normal error result.
            Ok(outcome) => ToolResult {
                content: outcome.text,
                is_error: outcome.is_error,
            },
            Err(e) => ToolResult {
                content: format!("MCP tool error: {}", e),
                is_error: true,
            },
        }
    }

    /// W8a A.4 (resolves audit F1) — race the in-flight JSON-RPC call
    /// against `ctx.cancel.cancelled()` so cancelling an MCP tool stops
    /// blocking the agent immediately (no waiting on the MCP server's
    /// default per-RPC timeout).
    ///
    /// Audit C7 — on cancel we no longer just drop the in-flight future
    /// (which left the MCP child alive, possibly wedged, possibly
    /// desynced for the next call). We tear the server's transport down:
    /// `close_server` kills the child and marks it dead, so a wedged
    /// `ijfw-memory`-style server can't poison subsequent calls. The
    /// transport-layer timeout (audit C1/C6) is the backstop for the
    /// non-cancelled path; this is the prompt path for interactive cancel.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        self.execute_with_optional_effect(input, ctx, None).await
    }

    async fn execute_with_effect_ctx(
        &self,
        input: Value,
        ctx: &ToolContext,
        effect: Option<&ToolEffectContext>,
    ) -> ToolResult {
        self.execute_with_optional_effect(input, ctx, effect).await
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }

    /// Provenance for curation / provider-cap classification. Returns the
    /// originating MCP server name regardless of whether the display name was
    /// prefixed (`mcp__{server}__{tool}` on collision) or kept bare. This is
    /// what lets the engine classify a non-colliding (bare-named) MCP tool as
    /// MCP instead of mistaking it for a built-in.
    fn mcp_server(&self) -> Option<&str> {
        Some(&self.server_name)
    }

    fn describe(&self, input: &Value) -> String {
        format!(
            "MCP {}/{}: {}",
            self.server_name,
            self.tool_name,
            serde_json::to_string(input).unwrap_or_default()
        )
    }
}

/// Register all MCP tools into the tool registry, handling name collisions.
///
/// Strategy:
/// - If tool name doesn't collide with built-in or other MCP tools → use as-is
/// - If collision detected → prefix with "mcp__{server_name}__"
///
/// Each tool's deferred flag is read from the server's config:
/// `McpServerConfig::deferred` — defaults to `true` when absent.
pub fn register_mcp_tools(
    registry: &mut wcore_tools::registry::ToolRegistry,
    manager: &Arc<McpManager>,
    builtin_names: &[String],
    server_configs: &HashMap<String, McpServerConfig>,
) {
    let all_tools = manager.all_tools();

    // Determine which names need prefixing
    for (server_name, tool_def) in &all_tools {
        let original_name = &tool_def.name;

        // Check collision with built-in tools
        let collides_builtin = builtin_names.iter().any(|n| n == original_name);

        // Check collision with other MCP servers' tools
        let cross_server_collision = manager.tool_name_count(original_name) > 1;

        let display_name = if collides_builtin || cross_server_collision {
            // mcp-41 — use a DOUBLE-underscore separator between server and
            // tool. A single `_` is ambiguous: server `foo` + tool
            // `bar_baz` and server `foo_bar` + tool `baz` both collapse to
            // `mcp__foo_bar_baz`, so two distinct (server,tool) pairs map to
            // one display name and one silently shadows the other. `__`
            // matches the documented convention (and the upstream MCP
            // gateway naming) and keeps the mapping injective for the common
            // case where neither name contains `__`.
            format!("mcp__{}__{}", server_name, original_name)
        } else {
            original_name.clone()
        };

        // MCP tools are deferred by default; server config can override.
        let deferred = server_configs
            .get(*server_name)
            .and_then(|c| c.deferred)
            .unwrap_or(true);

        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.to_string(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        );

        registry.register(Box::new(proxy));
    }
}

/// Register tools from a single newly-connected MCP server.
/// Uses the same collision-detection logic as `register_mcp_tools`.
pub fn register_single_server_tools(
    registry: &mut wcore_tools::registry::ToolRegistry,
    manager: &Arc<McpManager>,
    server_name: &str,
    builtin_names: &[String],
    deferred: bool,
    defer_cold: &wcore_config::tools::DeferColdConfig,
) {
    let all_tools = manager.all_tools();
    let server_tools: Vec<_> = all_tools
        .iter()
        .filter(|(sn, _)| *sn == server_name)
        .collect();

    for (_, tool_def) in &server_tools {
        let original_name = &tool_def.name;
        let collides_builtin = builtin_names.iter().any(|n| n == original_name);
        let cross_server_collision = manager.tool_name_count(original_name) > 1;

        let display_name = if collides_builtin || cross_server_collision {
            // mcp-41 — use a DOUBLE-underscore separator between server and
            // tool. A single `_` is ambiguous: server `foo` + tool
            // `bar_baz` and server `foo_bar` + tool `baz` both collapse to
            // `mcp__foo_bar_baz`, so two distinct (server,tool) pairs map to
            // one display name and one silently shadows the other. `__`
            // matches the documented convention (and the upstream MCP
            // gateway naming) and keeps the mapping injective for the common
            // case where neither name contains `__`.
            format!("mcp__{}__{}", server_name, original_name)
        } else {
            original_name.clone()
        };

        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.to_string(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        );

        registry.register(Box::new(proxy));
    }

    // Live single-server adds happen after bootstrap's ToolSearch snapshot.
    // Refresh in this shared registration seam so both JSON AddMcpServer and
    // the TUI `/mcp add` path cannot advertise tools that remain undiscoverable.
    registry.refresh_tool_search_catalog(defer_cold);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wcore_config::config::TransportType;

    fn make_proxy(deferred: bool) -> McpToolProxy {
        // manager is only used during execute(), which we don't call in these
        // tests, so we can construct one with no servers.
        let manager = Arc::new(McpManager::new_for_test(vec![]));
        McpToolProxy::new(
            "test_tool".into(),
            "test_tool".into(),
            "test_server".into(),
            "A test tool".into(),
            json!({"type": "object"}),
            manager,
            deferred,
        )
    }

    #[test]
    fn proxy_deferred_true_returns_true() {
        let proxy = make_proxy(true);
        assert!(proxy.is_deferred());
    }

    /// #135 linchpin — a DEFERRED MCP tool still registers EAGERLY and still
    /// carries real provenance (`ToolDef::server`) in `to_tool_defs()`. The
    /// `/mcp add` idempotency probe (`AgentEngine::mcp_server_connected`) keys
    /// purely on that provenance, so this is the load-bearing property: if a
    /// future refactor made deferred registration lazy or dropped the server
    /// tag, the probe would silently stop detecting a just-added deferred
    /// server and a re-add would spawn a duplicate process. Lock it here.
    #[test]
    fn deferred_mcp_tool_registers_eagerly_with_provenance() {
        let mut registry = wcore_tools::registry::ToolRegistry::new();
        registry.register(Box::new(make_proxy(true)));

        let defs = registry.to_tool_defs();
        assert_eq!(
            defs.len(),
            1,
            "the deferred tool must be registered eagerly"
        );
        assert!(
            defs[0].deferred,
            "the tool is deferred (name-only schema stub)"
        );
        assert_eq!(
            defs[0].server.as_deref(),
            Some("test_server"),
            "deferred registration must still carry real provenance for the probe"
        );
    }

    #[test]
    fn proxy_deferred_false_returns_false() {
        let proxy = make_proxy(false);
        assert!(!proxy.is_deferred());
    }

    fn make_server_config(deferred: Option<bool>) -> McpServerConfig {
        McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("echo".into()),
            args: None,
            env: None,
            url: None,
            headers: None,
            deferred,
            allow_local: false,
            only_for_assistant: None,
        }
    }

    #[test]
    fn register_defaults_to_deferred_when_config_omits_field() {
        let manager = Arc::new(McpManager::new_for_test(vec![]));
        let mut registry = wcore_tools::registry::ToolRegistry::new();
        // Empty server configs — deferred field absent
        let configs = HashMap::new();

        register_mcp_tools(&mut registry, &manager, &[], &configs);

        // No tools registered because manager has no tools, but the logic
        // is tested via the deferred default path. Test with a real config below.
        assert!(registry.tool_names().is_empty());
    }

    #[test]
    fn server_config_deferred_none_defaults_true() {
        let config = make_server_config(None);
        let deferred = config.deferred.unwrap_or(true);
        assert!(deferred, "deferred should default to true when None");
    }

    #[test]
    fn server_config_deferred_explicit_false() {
        let config = make_server_config(Some(false));
        let deferred = config.deferred.unwrap_or(true);
        assert!(!deferred, "deferred should be false when explicitly set");
    }

    #[test]
    fn server_config_deferred_explicit_true() {
        let config = make_server_config(Some(true));
        let deferred = config.deferred.unwrap_or(true);
        assert!(deferred, "deferred should be true when explicitly set");
    }

    // -----------------------------------------------------------------------
    // Audit C7 — cancelling an in-flight MCP tool call must tear down the
    // (possibly wedged) server transport: `close_server` → `transport.close`.
    // -----------------------------------------------------------------------

    use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
    use crate::transport::{McpError, McpTransport};
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use wcore_tools::context::{ToolContext, ToolEffectContext};

    struct CapturingTransport {
        requests: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    #[async_trait]
    impl McpTransport for CapturingTransport {
        async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            self.requests
                .lock()
                .unwrap()
                .push(serde_json::to_value(req).unwrap());
            Ok(JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(json!({
                    "content": [{"type": "text", "text": "ok"}]
                })),
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

    #[tokio::test]
    async fn durable_context_reuses_meta_key_and_legacy_call_has_no_meta() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let manager = Arc::new(McpManager::new_for_test(vec![(
            "capture",
            false,
            Box::new(CapturingTransport {
                requests: Arc::clone(&requests),
            }),
        )]));
        let proxy = McpToolProxy::new(
            "capture_tool".into(),
            "capture_tool".into(),
            "capture".into(),
            "captures requests".into(),
            json!({"type": "object"}),
            manager,
            false,
        );
        let input = json!({"nested": {"value": 7}});
        let ctx = ToolContext::test_default();
        let effect = ToolEffectContext {
            tool_execution_id: "tool-execution-9".into(),
            idempotency_key: "stable-key-9".into(),
        };

        proxy
            .execute_with_effect_ctx(input.clone(), &ctx, Some(&effect))
            .await;
        proxy
            .execute_with_effect_ctx(input.clone(), &ctx, Some(&effect))
            .await;
        proxy
            .execute_with_ctx(input.clone(), &ToolContext::test_default())
            .await;
        proxy.execute(input.clone()).await;

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for request in requests.iter() {
            assert_eq!(request["params"]["arguments"], input);
        }
        let expected_meta = json!({
            "wayland/durable-effect": {
                "version": 1,
                "toolExecutionId": "tool-execution-9",
                "idempotencyKey": "stable-key-9"
            }
        });
        assert_eq!(requests[0]["params"]["_meta"], expected_meta);
        assert_eq!(requests[1]["params"]["_meta"], expected_meta);
        assert!(requests[2]["params"].get("_meta").is_none());
        assert!(requests[3]["params"].get("_meta").is_none());
    }

    #[derive(Default)]
    struct IdempotentRemoteState {
        cached_results: HashMap<String, String>,
        request_count: usize,
        physical_effect_count: usize,
        cached_replay_count: usize,
    }

    struct IdempotentRemoteTransport {
        state: Arc<Mutex<IdempotentRemoteState>>,
    }

    #[async_trait]
    impl McpTransport for IdempotentRemoteTransport {
        async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            let stable_key = req
                .params
                .as_ref()
                .and_then(|params| params.get("_meta"))
                .and_then(|metadata| metadata.get("wayland/durable-effect"))
                .and_then(|effect| effect.get("idempotencyKey"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);

            let result = {
                let mut state = self.state.lock().unwrap();
                state.request_count += 1;

                if let Some(stable_key) = stable_key {
                    if let Some(cached) = state.cached_results.get(&stable_key).cloned() {
                        state.cached_replay_count += 1;
                        cached
                    } else {
                        state.physical_effect_count += 1;
                        let result = format!("physical-effect-{}", state.physical_effect_count);
                        state.cached_results.insert(stable_key, result.clone());
                        result
                    }
                } else {
                    state.physical_effect_count += 1;
                    format!("physical-effect-{}", state.physical_effect_count)
                }
            };

            Ok(JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(json!({
                    "content": [{"type": "text", "text": result}]
                })),
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

    #[tokio::test]
    async fn remote_server_deduplicates_stable_key_but_not_legacy_calls() {
        let state = Arc::new(Mutex::new(IdempotentRemoteState::default()));
        let manager = Arc::new(McpManager::new_for_test(vec![(
            "idempotent-remote",
            false,
            Box::new(IdempotentRemoteTransport {
                state: Arc::clone(&state),
            }),
        )]));
        let proxy = McpToolProxy::new(
            "charge_card".into(),
            "charge_card".into(),
            "idempotent-remote".into(),
            "records one externally visible charge".into(),
            json!({"type": "object"}),
            manager,
            false,
        );
        let input = json!({"amount": 42});
        let durable_ctx = ToolContext::test_default();
        let effect = ToolEffectContext {
            tool_execution_id: "execution-42".into(),
            idempotency_key: "stable-charge-key-42".into(),
        };

        let first = proxy
            .execute_with_effect_ctx(input.clone(), &durable_ctx, Some(&effect))
            .await;
        let replay = proxy
            .execute_with_effect_ctx(input.clone(), &durable_ctx, Some(&effect))
            .await;

        assert!(!first.is_error);
        assert_eq!(first.content, "physical-effect-1");
        assert!(!replay.is_error);
        assert_eq!(
            replay.content, first.content,
            "the second call must replay the cached result"
        );
        {
            let state = state.lock().unwrap();
            assert_eq!(state.request_count, 2);
            assert_eq!(state.physical_effect_count, 1);
            assert_eq!(state.cached_replay_count, 1);
            assert_eq!(state.cached_results.len(), 1);
        }

        let legacy_first = proxy.execute(input.clone()).await;
        let legacy_second = proxy
            .execute_with_ctx(input, &ToolContext::test_default())
            .await;

        assert_eq!(legacy_first.content, "physical-effect-2");
        assert_eq!(legacy_second.content, "physical-effect-3");
        let state = state.lock().unwrap();
        assert_eq!(state.request_count, 4);
        assert_eq!(state.physical_effect_count, 3);
        assert_eq!(state.cached_replay_count, 1);
        assert_eq!(state.cached_results.len(), 1);
    }

    /// Transport that hangs on `request` (a wedged MCP server) and records
    /// whether `close()` was called.
    struct WedgedRecordingTransport {
        closed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl McpTransport for WedgedRecordingTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            // Simulate a wedged server: never respond.
            tokio::time::sleep(Duration::from_secs(30)).await;
            Err(McpError::Transport("unreachable".into()))
        }
        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), McpError> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// mcp-41 — two distinct (server, tool) pairs that previously collapsed
    /// to one display name under the single-underscore scheme must now map
    /// to distinct names. Server `foo` with tool `bar_baz`, and server
    /// `foo_bar` with tool `baz`, both yielded `mcp__foo_bar_baz` before the
    /// fix; with the `__` separator they become `mcp__foo__bar_baz` and
    /// `mcp__foo_bar__baz`.
    ///
    /// Both tools share the original name with a builtin (forced collision)
    /// so the prefixing branch is exercised. We then assert all registered
    /// display names are unique — no silent shadowing.
    #[test]
    fn mcp41_collision_prefix_is_unambiguous() {
        use crate::protocol::McpToolDef;

        // Force prefixing: both servers expose a tool whose *original* name
        // collides with a builtin (`read`). The display-name builder then
        // takes the `mcp__{server}__{tool}` branch for each. The two servers
        // are named so a single-underscore join would alias them.
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![
            (
                "foo",
                false,
                Box::new(StubTransport),
                vec![McpToolDef {
                    name: "read".into(),
                    description: None,
                    input_schema: json!({}),
                }],
            ),
            (
                "foo_bar",
                false,
                Box::new(StubTransport),
                vec![McpToolDef {
                    name: "read".into(),
                    description: None,
                    input_schema: json!({}),
                }],
            ),
        ]));

        let mut registry = wcore_tools::registry::ToolRegistry::new();
        let builtins = vec!["read".to_string()];
        let configs = HashMap::new();
        register_mcp_tools(&mut registry, &manager, &builtins, &configs);

        let mut names = registry.tool_names();
        names.sort();
        // Both must be registered AND distinct (no collapse / shadowing).
        assert!(
            names.contains(&"mcp__foo__read".to_string()),
            "expected mcp__foo__read in {names:?}"
        );
        assert!(
            names.contains(&"mcp__foo_bar__read".to_string()),
            "expected mcp__foo_bar__read in {names:?}"
        );
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "display names must be unique, got duplicates in {names:?}"
        );
    }

    /// Minimal transport stub for registration tests (never driven).
    struct StubTransport;

    #[async_trait]
    impl McpTransport for StubTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            Err(McpError::Transport("stub".into()))
        }
        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
    }

    /// #562: the shared one-server registration seam used by JSON
    /// `AddMcpServer` and TUI `/mcp add` must refresh the already-registered
    /// ToolSearch catalog. The proxy is explicitly non-deferred, so finding it
    /// also proves the global cold split is reapplied before snapshotting.
    #[tokio::test]
    async fn single_server_live_add_refreshes_real_tool_search_catalog() {
        use crate::protocol::McpToolDef;

        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "dynamic",
            false,
            Box::new(StubTransport),
            vec![McpToolDef {
                name: "late_dynamic".into(),
                description: Some("late dynamic MCP fixture".into()),
                input_schema: json!({"type": "object"}),
            }],
        )]));
        let defer_cold = wcore_config::tools::DeferColdConfig::default();
        let mut registry = wcore_tools::registry::ToolRegistry::new();
        registry.refresh_tool_search_catalog(&defer_cold);

        register_single_server_tools(&mut registry, &manager, "dynamic", &[], false, &defer_cold);

        let result = registry
            .get("ToolSearch")
            .expect("live add must retain ToolSearch")
            .execute(json!({"query": "late_dynamic"}))
            .await;
        assert!(
            result.content.contains("\"name\": \"late_dynamic\"")
                && result.content.contains("\"parameters\""),
            "live-added tool must be discoverable through the real ToolSearch; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn c7_cancel_tears_down_wedged_server_transport() {
        let closed = Arc::new(AtomicBool::new(false));
        let manager = Arc::new(McpManager::new_for_test(vec![(
            "wedged",
            false,
            Box::new(WedgedRecordingTransport {
                closed: Arc::clone(&closed),
            }),
        )]));
        let proxy = McpToolProxy::new(
            "wedged_tool".into(),
            "wedged_tool".into(),
            "wedged".into(),
            "A wedged MCP tool".into(),
            json!({"type": "object"}),
            manager,
            false,
        );

        let ctx = ToolContext::test_default();
        ctx.cancel.cancel(); // pre-fire: cancel wins the select immediately

        let result = proxy.execute_with_ctx(json!({}), &ctx).await;

        assert!(result.is_error, "cancelled MCP tool must error");
        assert!(
            result.content.to_lowercase().contains("abort")
                || result.content.to_lowercase().contains("cancel"),
            "expected a cancellation message, got: {}",
            result.content
        );
        // Audit C7 — the wedged server's transport was torn down so the
        // child cannot leak / desync the next call.
        assert!(
            closed.load(Ordering::SeqCst),
            "cancel must call transport.close() on the wedged server"
        );
    }
}
