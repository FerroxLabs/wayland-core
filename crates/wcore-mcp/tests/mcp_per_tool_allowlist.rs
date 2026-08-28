//! #998 — the Desktop MCP Library's PER-TOOL switches must be honoured by core.
//!
//! Before this, `McpServerConfig` had no tool dimension at all: every tool a
//! server advertised was registered unconditionally, so a tool the operator
//! switched OFF in the Library stayed fully callable. The switch appeared to
//! work and did nothing — a control that lies.
//!
//! Four properties, each independently falsifiable:
//!
//!   1. a tool omitted from a declared `allowed_tools` is NOT registered, and
//!      so is not dispatchable (`registry.get()` is `None`), not merely hidden
//!      from the outbound schema;
//!   2. the denial SURVIVES a `notifications/tools/list_changed` refresh —
//!      `refresh_changed_mcp_tools` drops and re-registers a server wholesale,
//!      so an enforcement point placed only in the boot-time seam would be
//!      silently undone by the server's next notification;
//!   3. an ABSENT `allowed_tools` registers everything (the back-compat
//!      control: existing configs must be bit-for-bit unaffected);
//!   4. `allowed_tools: []` — the Library's "Disable all" — contributes no
//!      tools, and specifically does not fall back to allow-all.

#![cfg(feature = "test-utils")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde_json::json;

use wcore_mcp::manager::{McpManager, TestServerEntry};
use wcore_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef};
use wcore_mcp::transport::{McpError, McpTransport};

/// A server whose advertised tool list can grow, announced with the
/// `tools/list_changed` flag the real stdio reader raises off the wire.
struct GrowingTransport {
    tools: Mutex<Vec<String>>,
    tools_changed: AtomicBool,
}

impl GrowingTransport {
    fn new(initial: &[&str]) -> Self {
        Self {
            tools: Mutex::new(initial.iter().map(|n| n.to_string()).collect()),
            tools_changed: AtomicBool::new(false),
        }
    }

    fn register_and_announce(&self, name: &str) {
        self.tools.lock().unwrap().push(name.to_string());
        self.tools_changed.store(true, Ordering::SeqCst);
    }

    /// Announce a change that does not add anything, to drive a refresh over
    /// an unchanged-but-re-listed catalogue.
    fn announce_only(&self) {
        self.tools_changed.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl McpTransport for GrowingTransport {
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        assert_eq!(req.method, "tools/list");
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

fn tool_def(name: &str) -> McpToolDef {
    McpToolDef {
        name: name.to_string(),
        description: Some(name.to_string()),
        input_schema: json!({}),
    }
}

fn config_with(allowed: Option<Vec<String>>) -> wcore_config::config::McpServerConfig {
    config_for(wcore_config::config::TransportType::Stdio, allowed)
}

/// The same declaration under an explicit transport.
///
/// This matters for more than coverage. On the Wayland desktop, a **stdio**
/// connector's per-tool selection is ALSO enforced outside core, by a spawn
/// shim that re-exports only the allowed tools, so a stdio test can pass with
/// core doing nothing at all. **Hosted http/sse has no spawn to wrap**, so core
/// is the only enforcement that exists on those transports — and the config
/// file path is core's alone on every transport.
///
/// Nothing here spawns a process: the fixture transport below is an in-process
/// `McpTransport` impl, so no shim can be interposed and no pass can be
/// attributed to one. The transport is nonetheless varied explicitly, because
/// "the registration seam does not branch on transport" is a property worth
/// asserting rather than assuming.
fn config_for(
    transport: wcore_config::config::TransportType,
    allowed: Option<Vec<String>>,
) -> wcore_config::config::McpServerConfig {
    use wcore_config::config::TransportType;
    let hosted = !matches!(transport, TransportType::Stdio);
    wcore_config::config::McpServerConfig {
        command: (!hosted).then(|| "warehouse".to_string()),
        url: hosted.then(|| "https://warehouse.example/mcp".to_string()),
        transport,
        args: None,
        env: None,
        headers: None,
        deferred: Some(false),
        allow_local: false,
        only_for_assistant: None,
        allowed_tools: allowed,
    }
}

/// Build a one-server fixture advertising `inventory_reserve` + `payroll_wipe`
/// under `allowed`, returning the live transport, manager and configs.
fn fixture(
    allowed: Option<Vec<String>>,
) -> (
    Arc<GrowingTransport>,
    Arc<McpManager>,
    HashMap<String, wcore_config::config::McpServerConfig>,
) {
    let warehouse = Arc::new(GrowingTransport::new(&[
        "inventory_reserve",
        "payroll_wipe",
    ]));
    let entries: Vec<TestServerEntry> = vec![(
        "warehouse",
        false,
        Box::new(ArcTransport(warehouse.clone())) as Box<dyn McpTransport>,
        vec![tool_def("inventory_reserve"), tool_def("payroll_wipe")],
    )];
    let manager = Arc::new(McpManager::new_for_test_with_tools(entries));
    let mut configs = HashMap::new();
    configs.insert("warehouse".to_string(), config_with(allowed));
    (warehouse, manager, configs)
}

/// (1) A tool the operator switched off is not registered — and therefore not
/// dispatchable, not merely absent from the outbound schema.
#[test]
fn a_denied_tool_is_not_registered() {
    let (_t, manager, configs) = fixture(Some(vec!["inventory_reserve".to_string()]));
    let defer_cold = wcore_config::tools::DeferColdConfig::default();

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(&mut registry, &manager, &[], &configs, &defer_cold);

    assert!(
        registry.get("inventory_reserve").is_some(),
        "an ALLOWED tool must still register"
    );
    assert!(
        registry.get("payroll_wipe").is_none(),
        "a tool omitted from allowed_tools must not be dispatchable"
    );
    assert!(
        !registry
            .to_tool_defs()
            .iter()
            .any(|d| d.name == "payroll_wipe"),
        "a denied tool must not reach the outbound tool definitions either"
    );
}

/// (2) The mandatory `list_changed` round trip. `refresh_changed_mcp_tools`
/// drops the server and re-registers it wholesale, so enforcement placed only
/// in the boot seam would be undone by the server's very next notification —
/// and a server that ADDS a tool mid-session must not thereby acquire an
/// authority the operator never granted.
#[tokio::test]
async fn the_denial_survives_a_list_changed_refresh() {
    let (warehouse, manager, configs) = fixture(Some(vec!["inventory_reserve".to_string()]));
    let defer_cold = wcore_config::tools::DeferColdConfig::default();

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(&mut registry, &manager, &[], &configs, &defer_cold);
    assert!(registry.get("payroll_wipe").is_none());

    let refresh =
        wcore_mcp::tool_proxy::McpCatalogRefresh::new(vec![manager.clone()], Vec::new(), configs);

    // The server registers a NEW tool mid-session and announces it.
    warehouse.register_and_announce("payroll_export");
    let refreshed = refresh.apply(&mut registry, &defer_cold).await;
    assert_eq!(refreshed, vec!["warehouse".to_string()]);

    assert!(
        registry.get("inventory_reserve").is_some(),
        "the refresh must not drop the allowed tool"
    );
    assert!(
        registry.get("payroll_wipe").is_none(),
        "the wholesale drop-and-re-register must not resurrect a denied tool"
    );
    assert!(
        registry.get("payroll_export").is_none(),
        "a tool that appears mid-session and is not on the allow-list must \
         not become callable: silence in a declared list means OFF"
    );

    // A bare re-announcement (nothing added) must also stay enforced.
    warehouse.announce_only();
    refresh.apply(&mut registry, &defer_cold).await;
    assert!(registry.get("payroll_wipe").is_none());
}

/// (3) BACK-COMPAT CONTROL. An absent `allowed_tools` is "no selection made",
/// so every advertised tool registers exactly as it did before #998. If this
/// ever fails, the fix has silently narrowed every existing user's toolset.
#[tokio::test]
async fn an_absent_allowlist_registers_every_tool() {
    let (warehouse, manager, configs) = fixture(None);
    let defer_cold = wcore_config::tools::DeferColdConfig::default();

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(&mut registry, &manager, &[], &configs, &defer_cold);
    assert!(registry.get("inventory_reserve").is_some());
    assert!(registry.get("payroll_wipe").is_some());

    // ...and it stays allow-all across a refresh, so the two seams agree.
    let refresh =
        wcore_mcp::tool_proxy::McpCatalogRefresh::new(vec![manager.clone()], Vec::new(), configs);
    warehouse.register_and_announce("payroll_export");
    refresh.apply(&mut registry, &defer_cold).await;
    assert!(
        registry.get("payroll_export").is_some(),
        "with no selection declared, a late tool must still become callable"
    );
}

/// A server with NO config entry at all (a plugin- or host-owned connection)
/// is likewise unrestricted — the lookup must fail open, not closed.
#[test]
fn a_server_with_no_config_entry_registers_every_tool() {
    let (_t, manager, _configs) = fixture(None);
    let defer_cold = wcore_config::tools::DeferColdConfig::default();

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(
        &mut registry,
        &manager,
        &[],
        &HashMap::new(),
        &defer_cold,
    );
    assert!(registry.get("inventory_reserve").is_some());
    assert!(registry.get("payroll_wipe").is_some());
}

/// (4) The Library's "Disable all". An EMPTY list is a real selection that
/// happens to name nothing — it must not be confused with "absent".
#[test]
fn an_empty_allowlist_disables_every_tool() {
    let (_t, manager, configs) = fixture(Some(Vec::new()));
    let defer_cold = wcore_config::tools::DeferColdConfig::default();

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(&mut registry, &manager, &[], &configs, &defer_cold);
    assert!(registry.get("inventory_reserve").is_none());
    assert!(registry.get("payroll_wipe").is_none());
}

/// The predicate itself, over the three states, stated separately from the
/// registration paths so a future refactor of either cannot quietly redefine
/// what an operator meant. The empty case is the one that inverts: folding it
/// together with "absent" would enable every tool at exactly the moment the
/// operator asked for none.
#[test]
fn the_three_selection_states_stay_distinguishable() {
    use wcore_config::config::tool_allows;

    assert!(tool_allows(None, "anything"), "absent => allow all");

    let named = vec!["inventory_reserve".to_string()];
    assert!(tool_allows(Some(&named), "inventory_reserve"));
    assert!(
        !tool_allows(Some(&named), "payroll_wipe"),
        "a declared list denies by silence"
    );

    let none_selected: Vec<String> = Vec::new();
    assert!(
        !tool_allows(Some(&none_selected), "inventory_reserve"),
        "an EMPTY list means the operator disabled every tool, not that there \
         is no filter"
    );
}

/// The allow-list matches the name the SERVER advertises even when a builtin
/// collision forces the registered display name to `mcp__{server}__{tool}` —
/// otherwise a collision appearing would silently revoke an operator's grant.
#[test]
fn the_allowlist_matches_the_advertised_name_under_a_collision_prefix() {
    let (_t, manager, configs) = fixture(Some(vec!["inventory_reserve".to_string()]));
    let defer_cold = wcore_config::tools::DeferColdConfig::default();
    let builtins = vec!["inventory_reserve".to_string(), "payroll_wipe".to_string()];

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(
        &mut registry,
        &manager,
        &builtins,
        &configs,
        &defer_cold,
    );

    assert!(
        registry.get("mcp__warehouse__inventory_reserve").is_some(),
        "the grant must follow the advertised name through the collision prefix"
    );
    assert!(
        registry.get("mcp__warehouse__payroll_wipe").is_none(),
        "and so must the denial"
    );
}

/// CORRECTION 2/3 — the transport that has no other enforcement.
///
/// A hosted `streamable-http` (or `sse`) MCP server is never spawned, so the
/// desktop's stdio spawn shim cannot filter it. Whatever core registers is what
/// the model can call. This is the case where core's half of #998 is the ONLY
/// thing standing between a switched-off tool and a live dispatch, so it is
/// asserted directly rather than inferred from the stdio case.
#[test]
fn a_denied_tool_is_not_registered_on_a_hosted_http_server() {
    use wcore_config::config::TransportType;

    for transport in [TransportType::StreamableHttp, TransportType::Sse] {
        let (_t, manager, _) = fixture(None);
        let mut configs = HashMap::new();
        configs.insert(
            "warehouse".to_string(),
            config_for(
                transport.clone(),
                Some(vec!["inventory_reserve".to_string()]),
            ),
        );
        let defer_cold = wcore_config::tools::DeferColdConfig::default();

        let mut registry = wcore_tools::registry::ToolRegistry::new();
        wcore_mcp::tool_proxy::register_mcp_tools(
            &mut registry,
            &manager,
            &[],
            &configs,
            &defer_cold,
        );

        assert!(
            registry.get("inventory_reserve").is_some(),
            "{transport:?}: the allowed tool must still register"
        );
        assert!(
            registry.get("payroll_wipe").is_none(),
            "{transport:?}: core is the ONLY enforcement on a hosted transport - \
             there is no spawn to interpose a filter on"
        );
    }
}

/// "Disable all" on a hosted transport, which is the state that reaches core
/// through the config file. Nothing else can withhold these tools.
#[test]
fn an_empty_allowlist_disables_every_tool_on_a_hosted_http_server() {
    let (_t, manager, _) = fixture(None);
    let mut configs = HashMap::new();
    configs.insert(
        "warehouse".to_string(),
        config_for(
            wcore_config::config::TransportType::StreamableHttp,
            Some(Vec::new()),
        ),
    );
    let defer_cold = wcore_config::tools::DeferColdConfig::default();

    let mut registry = wcore_tools::registry::ToolRegistry::new();
    wcore_mcp::tool_proxy::register_mcp_tools(&mut registry, &manager, &[], &configs, &defer_cold);

    assert!(registry.get("inventory_reserve").is_none());
    assert!(registry.get("payroll_wipe").is_none());
}

/// The registration decision must not depend on the transport at all. Asserted
/// rather than assumed: if a future change ever made enforcement conditional on
/// a spawn being available, hosted servers would silently lose their only
/// filter and this is the test that would say so.
#[test]
fn the_decision_is_identical_across_every_transport() {
    use wcore_config::config::TransportType;

    let registered_under = |transport: TransportType| -> Vec<String> {
        let (_t, manager, _) = fixture(None);
        let mut configs = HashMap::new();
        configs.insert(
            "warehouse".to_string(),
            config_for(transport, Some(vec!["inventory_reserve".to_string()])),
        );
        let defer_cold = wcore_config::tools::DeferColdConfig::default();
        let mut registry = wcore_tools::registry::ToolRegistry::new();
        wcore_mcp::tool_proxy::register_mcp_tools(
            &mut registry,
            &manager,
            &[],
            &configs,
            &defer_cold,
        );
        let mut names: Vec<String> = registry
            .to_tool_defs()
            .into_iter()
            .filter(|t| t.server.as_deref() == Some("warehouse"))
            .map(|t| t.name)
            .collect();
        names.sort();
        names
    };

    let stdio = registered_under(TransportType::Stdio);
    assert_eq!(stdio, vec!["inventory_reserve".to_string()]);
    assert_eq!(registered_under(TransportType::StreamableHttp), stdio);
    assert_eq!(registered_under(TransportType::Sse), stdio);
}
