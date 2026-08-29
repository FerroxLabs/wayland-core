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

// ---------------------------------------------------------------------------
// FerroxLabs/wayland#1174 + #1175 — a catalog refresh that starts empty (the
// `defer_config_mcp` mode the desktop host runs in) and a server attached
// after boot must BOTH honour `tools/list_changed`.
// ---------------------------------------------------------------------------

fn growing_manager(
    server: &'static str,
    initial: &[&str],
) -> (Arc<GrowingTransport>, Arc<McpManager>) {
    let transport = Arc::new(GrowingTransport::new(initial));
    let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
        server,
        false,
        Box::new(ArcTransport(transport.clone())) as Box<dyn McpTransport>,
        initial.iter().map(|n| tool_def(n)).collect(),
    )]));
    (transport, manager)
}

/// #1174. Under `defer_config_mcp` the boot-time refresh has no managers at
/// all, so the engine used to install nothing and every server in the session
/// lost `tools/list_changed`. The refresh must survive being empty and pick up
/// the manager the deferred connect produces.
#[tokio::test]
async fn a_refresh_that_started_empty_serves_the_deferred_config_connect() {
    let defer_cold = wcore_config::tools::DeferColdConfig::default();
    let mut registry = wcore_tools::registry::ToolRegistry::new();

    // Exactly what bootstrap builds when `defer_config_mcp` is set.
    let refresh = wcore_mcp::tool_proxy::McpCatalogRefresh::new(
        Vec::new(),
        vec!["Read".to_string()],
        HashMap::new(),
    );
    assert!(refresh.apply(&mut registry, &defer_cold).await.is_empty());

    // …and now the deferred connect lands.
    let (transport, manager) = growing_manager("warehouse", &["inventory_reserve"]);
    let mut configs = HashMap::new();
    configs.insert("warehouse".to_string(), stdio_config(false));
    wcore_mcp::tool_proxy::register_mcp_tools(
        &mut registry,
        &manager,
        &["Read".to_string()],
        &configs,
        &defer_cold,
    );
    refresh.register_runtime_server(&manager, &configs);

    transport.register_and_announce("inventory_audit_export");
    assert_eq!(
        refresh.apply(&mut registry, &defer_cold).await,
        vec!["warehouse".to_string()],
        "a deferred-config server must have its tools/list_changed honoured"
    );
    assert!(
        registry.get("inventory_audit_export").is_some(),
        "the late tool must be callable"
    );
}

/// #1175. `/mcp add` builds a brand-new manager; it must join the refresh.
#[tokio::test]
async fn a_runtime_added_server_is_refreshed_alongside_the_boot_servers() {
    let defer_cold = wcore_config::tools::DeferColdConfig::default();
    let builtin: Vec<String> = vec!["Read".to_string()];
    let mut registry = wcore_tools::registry::ToolRegistry::new();

    let (boot_tx, boot_mgr) = growing_manager("boot", &["boot_tool"]);
    let mut boot_configs = HashMap::new();
    boot_configs.insert("boot".to_string(), stdio_config(false));
    wcore_mcp::tool_proxy::register_mcp_tools(
        &mut registry,
        &boot_mgr,
        &builtin,
        &boot_configs,
        &defer_cold,
    );
    let refresh = wcore_mcp::tool_proxy::McpCatalogRefresh::new(
        vec![boot_mgr.clone()],
        builtin.clone(),
        boot_configs,
    );

    let (live_tx, live_mgr) = growing_manager("live", &["live_tool"]);
    let mut live_configs = HashMap::new();
    live_configs.insert("live".to_string(), stdio_config(false));
    wcore_mcp::tool_proxy::register_single_server_tools(
        &mut registry,
        &live_mgr,
        "live",
        &builtin,
        true,
        None,
        &defer_cold,
    );
    refresh.register_runtime_server(&live_mgr, &live_configs);

    live_tx.register_and_announce("live_late_tool");
    boot_tx.register_and_announce("boot_late_tool");
    let mut refreshed = refresh.apply(&mut registry, &defer_cold).await;
    refreshed.sort();
    assert_eq!(refreshed, vec!["boot".to_string(), "live".to_string()]);
    assert!(
        registry.get("live_late_tool").is_some(),
        "a server added with /mcp add must not be opted out of tool updates"
    );
    // Negative control: the boot server is not broken by the runtime add.
    assert!(registry.get("boot_late_tool").is_some());
    assert!(registry.get("boot_tool").is_some());
}

/// #998 must not regress through the new door. An operator allowlist of
/// `Some([])` means "disable every tool on this server" — at boot, on the
/// live add, AND after a `list_changed`. Registering the manager without its
/// config would reach the `config == None -> allow-all` read in
/// `tool_proxy.rs`, which is why `register_runtime_server` takes the configs.
#[tokio::test]
async fn a_runtime_added_servers_empty_allowlist_survives_a_refresh() {
    let defer_cold = wcore_config::tools::DeferColdConfig::default();
    let builtin: Vec<String> = Vec::new();
    let mut registry = wcore_tools::registry::ToolRegistry::new();

    let (transport, manager) = growing_manager("locked", &["locked_tool"]);
    let mut configs = HashMap::new();
    let mut config = stdio_config(false);
    config.allowed_tools = Some(Vec::new());
    configs.insert("locked".to_string(), config);

    wcore_mcp::tool_proxy::register_single_server_tools(
        &mut registry,
        &manager,
        "locked",
        &builtin,
        true,
        Some(&[]),
        &defer_cold,
    );
    assert!(
        registry.get("locked_tool").is_none(),
        "boot: allowlist is empty"
    );

    let refresh =
        wcore_mcp::tool_proxy::McpCatalogRefresh::new(Vec::new(), builtin, HashMap::new());
    refresh.register_runtime_server(&manager, &configs);

    transport.register_and_announce("locked_late_tool");
    assert_eq!(
        refresh.apply(&mut registry, &defer_cold).await,
        vec!["locked".to_string()]
    );
    assert!(
        registry.get("locked_late_tool").is_none(),
        "an empty allowlist must still mean 'no tools' after a list_changed"
    );
    assert!(registry.get("locked_tool").is_none());
}

/// The structural guard behind that: a manager offered with no config for the
/// server it serves is REFUSED, not silently admitted as allow-all.
#[tokio::test]
async fn a_runtime_manager_with_no_config_is_refused() {
    let defer_cold = wcore_config::tools::DeferColdConfig::default();
    let mut registry = wcore_tools::registry::ToolRegistry::new();
    let (transport, manager) = growing_manager("orphan", &["orphan_tool"]);

    let refresh =
        wcore_mcp::tool_proxy::McpCatalogRefresh::new(Vec::new(), Vec::new(), HashMap::new());
    assert!(
        !refresh.register_runtime_server(&manager, &HashMap::new()),
        "a manager with no server config must not enter the refresh"
    );

    transport.register_and_announce("orphan_late_tool");
    assert!(
        refresh.apply(&mut registry, &defer_cold).await.is_empty(),
        "a refused manager must not be polled"
    );
}

/// A runtime add that is rolled back (the TUI's `!published` path) must leave
/// nothing behind: neither the manager nor its config.
#[tokio::test]
async fn a_withdrawn_runtime_server_stops_being_refreshed() {
    let defer_cold = wcore_config::tools::DeferColdConfig::default();
    let mut registry = wcore_tools::registry::ToolRegistry::new();
    let (transport, manager) = growing_manager("transient", &["transient_tool"]);
    let mut configs = HashMap::new();
    configs.insert("transient".to_string(), stdio_config(false));

    let refresh =
        wcore_mcp::tool_proxy::McpCatalogRefresh::new(Vec::new(), Vec::new(), HashMap::new());
    assert!(refresh.register_runtime_server(&manager, &configs));
    refresh.forget_runtime_server("transient");

    transport.register_and_announce("transient_late_tool");
    assert!(refresh.apply(&mut registry, &defer_cold).await.is_empty());
    assert!(registry.get("transient_late_tool").is_none());
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
