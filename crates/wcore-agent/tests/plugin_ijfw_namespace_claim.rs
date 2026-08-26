//! `wayland-ijfw` registers `ijfw_run` / `ijfw_update_apply` as `PluginTool`s
//! whose closures can only ever return an error — the same defect class as the
//! `wayland-browser` / `wayland-cua` bare-`execute` claims.
//!
//! The plugin's own module doc states it outright: *"The `PluginTool`
//! registered here is therefore a host-delegated namespace claim: it carries
//! honest metadata, and its closure is never the live execution path."* The
//! real tools are served by the `ijfw-memory` MCP server and surfaced by
//! `wcore-mcp`'s tool proxy.
//!
//! For IJFW this is WORSE than the browser/cua case, because of registration
//! order in `bootstrap.rs`:
//!
//!   1. `let builtin_names = registry.tool_names()` is snapshotted at :1709,
//!      BEFORE any plugin tool is delivered.
//!   2. `apply_initialize_outcome` delivers the inert `ijfw_run` at :1756.
//!   3. `register_mcp_tools` runs at :1810. Its collision test is against that
//!      pre-plugin `builtin_names` snapshot, so it does NOT see the plugin's
//!      `ijfw_run`, finds no collision, and registers the REAL tool under the
//!      same bare name.
//!   4. `ToolRegistry::register` pushes onto a `Vec` and `get()` returns the
//!      FIRST match — which is the inert plugin tool from step 2.
//!
//! So the inert claim does not merely sit beside the real tool: it SHADOWS it.
//! `to_tool_defs()` also advertises both, offering the model the same name
//! twice.
//!
//! These tests drive the real `WaylandIjfwFactory` through the production
//! `PluginRunner` → `apply_initialize_outcome` path, not a fixture, so the
//! marker cannot be dropped from `wayland-ijfw` without failing here.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_agent::plugins::{DiscoveredPlugin, PluginRunner, apply_initialize_outcome};
use wcore_plugin_api::PluginFactory;
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::tool::{JsonSchema, ToolResult};

/// Stands in for the `McpToolProxy` that `register_mcp_tools` builds for the
/// `ijfw-memory` server. Registered AFTER plugin delivery, under the BARE
/// name, exactly as `tool_proxy.rs` does when its `builtin_names` snapshot
/// shows no collision. Using a stand-in keeps the test hermetic — the
/// behaviour under test is registry ordering, not MCP transport.
struct FakeMcpProxy(&'static str);

#[async_trait]
impl Tool for FakeMcpProxy {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "real IJFW tool, served by the ijfw-memory MCP server"
    }
    fn input_schema(&self) -> JsonSchema {
        json!({ "type": "object", "properties": {} })
    }
    async fn execute(&self, _input: Value) -> ToolResult {
        ToolResult {
            content: "served by the MCP proxy".to_string(),
            is_error: false,
        }
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
}

fn discovered(factory: &'static dyn PluginFactory) -> DiscoveredPlugin {
    let plugin = factory.build();
    let manifest = plugin.manifest().clone();
    DiscoveredPlugin {
        name: factory.name().to_string(),
        manifest,
        plugin,
    }
}

/// Boot the real IJFW plugin through the production path, then register the
/// MCP-side tools the way `bootstrap.rs` does — after plugin delivery, bare.
fn boot_ijfw_then_mcp() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    rt.block_on(async {
        let plugins = vec![discovered(&wayland_ijfw::WaylandIjfwFactory)];
        let mut runner = PluginRunner::new();
        let outcome = runner
            .initialize_all(&plugins)
            .await
            .expect("initialize_all must not abort the boot");
        apply_initialize_outcome(outcome, &mut registry, runner.browser, runner.cua);
    });

    for name in wayland_ijfw::tools::TOOL_NAMES {
        registry.register(Box::new(FakeMcpProxy(name)));
    }
    registry
}

/// The headline defect: the inert plugin claim wins `get()` over the real
/// MCP-served tool, so every call to `ijfw_run` returns an error.
#[test]
fn the_inert_claim_does_not_shadow_the_real_mcp_tool() {
    let registry = boot_ijfw_then_mcp();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");

    for name in wayland_ijfw::tools::TOOL_NAMES {
        let tool = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} must resolve to something"));
        let result = rt.block_on(tool.execute(json!({})));
        assert!(
            !result.is_error,
            "`{name}` resolved to a tool whose only possible result is an \
             error — the inert plugin claim is shadowing the real MCP-served \
             tool. content: {}",
            result.content
        );
    }
}

/// The model must never be offered a name it cannot successfully call, and
/// must never be offered the same name twice.
#[test]
fn ijfw_claims_are_not_advertised_to_the_model() {
    let registry = boot_ijfw_then_mcp();
    let advertised: Vec<String> = registry
        .to_tool_defs()
        .into_iter()
        .map(|d| d.name)
        .collect();

    for name in wayland_ijfw::tools::TOOL_NAMES {
        let count = advertised.iter().filter(|n| n.as_str() == *name).count();
        assert_eq!(
            count, 1,
            "`{name}` must be advertised exactly once (the real MCP tool); \
             a second entry is the inert claim. advertised: {advertised:?}"
        );
    }
}

/// The claim must still be CAPTURED, so the plugin-side `NamespaceLedger`
/// duplicate-claim protection that `register_tool` provides is untouched.
/// Only what the host does with the claim at delivery changes.
#[test]
fn the_ijfw_claims_are_still_captured_before_delivery_drops_them() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    let outcome = rt.block_on(async {
        let plugins = vec![discovered(&wayland_ijfw::WaylandIjfwFactory)];
        let mut runner = PluginRunner::new();
        runner
            .initialize_all(&plugins)
            .await
            .expect("initialize_all must not abort the boot")
    });

    let mut claims: Vec<&str> = outcome
        .tools
        .iter()
        .filter(|c| c.tool.namespace_claim)
        .map(|c| c.fq_name.as_str())
        .collect();
    claims.sort_unstable();
    assert_eq!(
        claims,
        vec!["ijfw::ijfw_run", "ijfw::ijfw_update_apply"],
        "wayland-ijfw must still claim both names in its own namespace"
    );
}

/// NEGATIVE CONTROL for the IJFW change: dropping the claims must not drop
/// any OTHER surface the anchor plugin registers. `wayland-ijfw` exists to
/// exercise every `register_*` surface; a fix that silently disarmed one of
/// them would still pass the three tests above.
#[test]
fn dropping_the_claims_leaves_the_other_plugin_surfaces_intact() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    let outcome = rt.block_on(async {
        let plugins = vec![discovered(&wayland_ijfw::WaylandIjfwFactory)];
        let mut runner = PluginRunner::new();
        runner
            .initialize_all(&plugins)
            .await
            .expect("initialize_all must not abort the boot")
    });

    let hook_names: Vec<&str> = outcome.hooks.iter().map(|h| h.name.as_str()).collect();
    for (_, expected) in wayland_ijfw::hooks::HOOKS {
        assert!(
            hook_names.contains(expected),
            "hook `{expected}` must survive; got {hook_names:?}"
        );
    }
    assert!(!outcome.agents.is_empty(), "agents surface must survive");
    assert!(!outcome.skills.is_empty(), "skills surface must survive");
    assert!(!outcome.rules.is_empty(), "rules surface must survive");
}
