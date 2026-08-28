//! A-11 — a tool that appears MID-SESSION must become callable.
//!
//! An MCP server may register a tool after the session has started and
//! announce it with `notifications/tools/list_changed` (the `tools.listChanged`
//! capability exists for exactly this). Tool discovery used to be one-shot at
//! connect, so such a tool reached neither `all_tools()` nor the live tool
//! registry, and was uncallable for the rest of the session however clearly
//! the server announced it.
//!
//! Two halves, tested separately:
//!   * the MANAGER re-lists a server that signalled, and only that server;
//!   * the live tool REGISTRY — what the model is offered and what dispatch
//!     resolves against — picks the new tool up.
//!
//! The transport half (observing the id-less notification off the wire) is
//! covered by the `sh`-fixture tests in `transport/stdio.rs`.

#![cfg(feature = "test-utils")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::json;

use wcore_mcp::manager::{McpManager, TestServerEntry};
use wcore_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef};
use wcore_mcp::transport::{McpError, McpTransport};

/// A server whose advertised tool list grows once, announced by raising the
/// `tools/list_changed` flag the real stdio reader raises off the wire.
struct GrowingTransport {
    /// Names the next `tools/list` will answer with.
    tools: Mutex<Vec<String>>,
    tools_changed: AtomicBool,
    /// How many `tools/list` requests actually went to the wire, so an idle
    /// poll can be proven to cost no traffic.
    lists: AtomicUsize,
}

impl GrowingTransport {
    fn new(initial: &[&str]) -> Self {
        Self {
            tools: Mutex::new(initial.iter().map(|n| n.to_string()).collect()),
            tools_changed: AtomicBool::new(false),
            lists: AtomicUsize::new(0),
        }
    }

    /// Register a tool mid-session and announce it, exactly as the warehouse
    /// fixture does after its first successful despatch.
    fn register_and_announce(&self, name: &str) {
        self.tools.lock().unwrap().push(name.to_string());
        self.tools_changed.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl McpTransport for GrowingTransport {
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        assert_eq!(
            req.method, "tools/list",
            "only tools/list is exercised here"
        );
        self.lists.fetch_add(1, Ordering::SeqCst);
        let tools: Vec<serde_json::Value> = self
            .tools
            .lock()
            .unwrap()
            .iter()
            .map(|name| json!({"name": name, "description": name, "inputSchema": {}}))
            .collect();
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(json!({ "tools": tools })),
            error: None,
        })
    }

    async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
        Ok(())
    }

    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }

    fn take_tools_changed(&self) -> bool {
        self.tools_changed.swap(false, Ordering::SeqCst)
    }
}

fn tool_def(name: &str) -> McpToolDef {
    McpToolDef {
        name: name.to_string(),
        description: Some(name.to_string()),
        input_schema: json!({}),
    }
}

fn tool_names(manager: &McpManager) -> Vec<String> {
    let mut names: Vec<String> = manager
        .all_tools()
        .into_iter()
        .map(|(_, t)| t.name)
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a11_manager_relists_only_the_server_that_signalled() {
    let warehouse = Arc::new(GrowingTransport::new(&["inventory_reserve"]));
    let quiet = Arc::new(GrowingTransport::new(&["unrelated"]));

    let entries: Vec<TestServerEntry> = vec![
        (
            "warehouse",
            false,
            Box::new(ArcTransport(warehouse.clone())) as Box<dyn McpTransport>,
            vec![tool_def("inventory_reserve")],
        ),
        (
            "quiet",
            false,
            Box::new(ArcTransport(quiet.clone())) as Box<dyn McpTransport>,
            vec![tool_def("unrelated")],
        ),
    ];
    let manager = McpManager::new_for_test_with_tools(entries);

    // Nothing has signalled: an idle poll must send no traffic at all.
    assert!(manager.refresh_signalled_tools().await.is_empty());
    assert_eq!(warehouse.lists.load(Ordering::SeqCst), 0);
    assert_eq!(quiet.lists.load(Ordering::SeqCst), 0);
    assert_eq!(tool_names(&manager), vec!["inventory_reserve", "unrelated"]);

    // The warehouse registers the export tool after the first despatch.
    warehouse.register_and_announce("inventory_audit_export");

    let refreshed = manager.refresh_signalled_tools().await;
    assert_eq!(refreshed, vec!["warehouse".to_string()]);
    assert_eq!(
        warehouse.lists.load(Ordering::SeqCst),
        1,
        "the signalling server must be re-listed exactly once"
    );
    assert_eq!(
        quiet.lists.load(Ordering::SeqCst),
        0,
        "a server that said nothing must not be re-listed"
    );

    assert_eq!(
        tool_names(&manager),
        vec!["inventory_audit_export", "inventory_reserve", "unrelated"]
    );
    assert!(manager.has_tool_name("inventory_audit_export"));

    // The signal is take-and-cleared: a second poll is a no-op.
    assert!(manager.refresh_signalled_tools().await.is_empty());
    assert_eq!(warehouse.lists.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a11_late_tool_reaches_the_live_tool_registry() {
    let warehouse = Arc::new(GrowingTransport::new(&["inventory_reserve"]));
    let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "warehouse",
        false,
        Box::new(ArcTransport(warehouse.clone())) as Box<dyn McpTransport>,
        vec![tool_def("inventory_reserve")],
    )]));

    let mut server_configs = HashMap::new();
    server_configs.insert("warehouse".to_string(), stdio_config(false));
    let defer_cold = wcore_config::tools::DeferColdConfig::default();
    let builtin_names: Vec<String> = vec!["Read".to_string(), "Bash".to_string()];

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(
        &mut registry,
        &manager,
        &builtin_names,
        &server_configs,
        &defer_cold,
    );
    assert!(registry.get("inventory_reserve").is_some());
    assert!(
        registry.get("inventory_audit_export").is_none(),
        "the export tool does not exist when the session starts"
    );

    let refresh = wcore_mcp::tool_proxy::McpCatalogRefresh::new(
        vec![manager.clone()],
        builtin_names,
        server_configs,
    );

    // No signal yet — the registry must be left exactly as it was.
    assert!(refresh.apply(&mut registry, &defer_cold).await.is_empty());
    assert!(registry.get("inventory_audit_export").is_none());

    warehouse.register_and_announce("inventory_audit_export");

    let refreshed = refresh.apply(&mut registry, &defer_cold).await;
    assert_eq!(refreshed, vec!["warehouse".to_string()]);
    assert!(
        registry.get("inventory_audit_export").is_some(),
        "a tool the server registered mid-session must be dispatchable"
    );
    assert!(
        registry.get("inventory_reserve").is_some(),
        "the refresh must not drop the tools that were already there"
    );

    // ...and it must be DISCOVERABLE, not merely present: the ToolSearch
    // snapshot is the model's only route to a deferred MCP tool.
    let defs = registry.to_tool_defs();
    assert!(
        defs.iter().any(|d| d.name == "inventory_audit_export"),
        "the late tool must reach the outbound tool definitions"
    );
    let search = registry
        .get("ToolSearch")
        .expect("registration refreshes the ToolSearch catalogue");
    assert!(
        search.description().contains("inventory_audit_export")
            || defs
                .iter()
                .any(|d| d.name == "inventory_audit_export" && !d.deferred),
        "the late tool must be either searchable or shipped in full"
    );
}

#[tokio::test]
async fn a11_a_removed_tool_stops_being_dispatchable() {
    let warehouse = Arc::new(GrowingTransport::new(&["inventory_reserve", "temporary"]));
    let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "warehouse",
        false,
        Box::new(ArcTransport(warehouse.clone())) as Box<dyn McpTransport>,
        vec![tool_def("inventory_reserve"), tool_def("temporary")],
    )]));

    let mut server_configs = HashMap::new();
    server_configs.insert("warehouse".to_string(), stdio_config(false));
    let defer_cold = wcore_config::tools::DeferColdConfig::default();
    let builtin_names: Vec<String> = Vec::new();

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(
        &mut registry,
        &manager,
        &builtin_names,
        &server_configs,
        &defer_cold,
    );
    assert!(registry.get("temporary").is_some());

    // `list_changed` also covers REMOVAL. A proxy left behind for a tool the
    // server no longer serves is a call that fails at the far end.
    warehouse.tools.lock().unwrap().retain(|n| n != "temporary");
    warehouse.tools_changed.store(true, Ordering::SeqCst);

    let refresh = wcore_mcp::tool_proxy::McpCatalogRefresh::new(
        vec![manager.clone()],
        builtin_names,
        server_configs,
    );
    assert_eq!(
        refresh.apply(&mut registry, &defer_cold).await,
        vec!["warehouse".to_string()]
    );
    assert!(registry.get("temporary").is_none());
    assert!(registry.get("inventory_reserve").is_some());
}

/// Lets one fixture be observed by the test while the manager owns a
/// `Box<dyn McpTransport>`.
struct ArcTransport(Arc<GrowingTransport>);

#[async_trait]
impl McpTransport for ArcTransport {
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        self.0.request(req).await
    }

    async fn notify(&self, req: &JsonRpcRequest) -> Result<(), McpError> {
        self.0.notify(req).await
    }

    async fn close(&self) -> Result<(), McpError> {
        self.0.close().await
    }

    fn take_tools_changed(&self) -> bool {
        self.0.take_tools_changed()
    }
}

fn stdio_config(deferred: bool) -> wcore_config::config::McpServerConfig {
    wcore_config::config::McpServerConfig {
        transport: wcore_config::config::TransportType::Stdio,
        command: Some("warehouse".to_string()),
        args: None,
        env: None,
        url: None,
        headers: None,
        deferred: Some(deferred),
        allow_local: false,
        only_for_assistant: None,
        allowed_tools: None,
    }
}
