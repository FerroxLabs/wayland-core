//! v0.6.4 Task 1.8 — Phase 1 exit gate: end-to-end plugin-tool round-trip.
//!
//! Proves the full pipeline that Tasks 1.1–1.7 wired together:
//!
//!   1. a real `Plugin` impl is discovered via `inventory::submit!`
//!      (Task 1.1 + plugin-api discovery),
//!   2. `PluginRunner::initialize_all` runs the plugin's `initialize()` and
//!      captures its `PluginTool` registration into `InitializeOutcome.tools`
//!      (Task 1.1 capture-then-reify contract),
//!   3. `apply_initialize_outcome` (Task 1.7) reifies the captured tool via
//!      `PluginToolAdapter` and inserts it into the engine `ToolRegistry`,
//!   4. the tool is then invocable by name from the registry — `execute()`
//!      runs the plugin closure and returns its `ToolResult`.
//!
//! This is the closing proof for Phase 1: capture → carry → deliver →
//! invoke, with no scaffolding in the middle. The earlier integration tests
//! (`plugin_api_smoke.rs`, `plugin_bootstrap_wiring.rs`, `plugin_tool_delivery.rs`)
//! each exercise one segment of the pipeline; this one walks it end to end
//! from a real `Plugin` to a callable tool.

use std::sync::Arc;

use wcore_agent::plugins::{PluginLoader, PluginRunner, apply_initialize_outcome};
use wcore_config::plugins_config::PluginsConfig;
use wcore_plugin_api::tool::{PluginTool, PluginToolInvocation};
use wcore_plugin_api::{Plugin, PluginContext, PluginFactory, PluginManifest, PluginResult};
use wcore_protocol::events::ToolCategory;
use wcore_tools::registry::ToolRegistry;
use wcore_types::tool::ToolResult;

// ── fixture plugin ──────────────────────────────────────────────────────────

static MANIFEST_TOML: &str = r#"
[plugin]
name = "wayland-e2e-fixture"
version = "0.0.1"
description = "Task 1.8 e2e fixture — registers echo_fixture"
entry = "builtin:e2e"
authors = ["t"]
license = "MIT"
[permissions]
register_tools = true
tool_namespace = "e2e"
"#;

fn fixture_manifest() -> &'static PluginManifest {
    static M: std::sync::OnceLock<PluginManifest> = std::sync::OnceLock::new();
    M.get_or_init(|| PluginManifest::from_toml_str(MANIFEST_TOML).expect("manifest parses"))
}

/// A real `PluginTool` whose execute closure echoes the `text` field — the
/// observable behavior we assert at the end of the pipeline.
fn echo_fixture_tool() -> PluginTool {
    PluginTool {
        name: "echo_fixture".into(),
        description: "echoes the `text` input field".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"],
        }),
        category: ToolCategory::Info,
        is_deferred: false,
        max_result_size: 4_096,
        execute: Arc::new(|inv: PluginToolInvocation| {
            Box::pin(async move {
                let text = inv
                    .input
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                ToolResult {
                    content: text,
                    is_error: false,
                }
            })
        }),
    }
}

struct E2eFixturePlugin;

#[async_trait::async_trait]
impl Plugin for E2eFixturePlugin {
    fn manifest(&self) -> &PluginManifest {
        fixture_manifest()
    }

    async fn initialize(&self, ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        ctx.tools
            .as_mut()
            .expect("register_tools=true grants ScopedToolRegistry")
            .register_tool(echo_fixture_tool())?;
        Ok(())
    }
}

struct E2eFixtureFactory;
impl PluginFactory for E2eFixtureFactory {
    fn name(&self) -> &'static str {
        "wayland-e2e-fixture"
    }
    fn build(&self) -> Box<dyn Plugin> {
        Box::new(E2eFixturePlugin)
    }
}
inventory::submit! { &E2eFixtureFactory as &dyn wcore_plugin_api::PluginFactory }

// ── the end-to-end test ─────────────────────────────────────────────────────

/// Phase 1 exit gate: a `Plugin` registered via the public surface delivers a
/// working tool that the engine can invoke by name.
#[tokio::test]
async fn registered_plugin_tool_is_invocable_through_the_full_pipeline() {
    // 1. Discover plugins (the fixture is `inventory::submit!`ed above).
    let cfg = PluginsConfig::default();
    let mut loader = PluginLoader::discover(&cfg);
    let discovered = loader.validate_all().expect("validate");
    assert!(
        discovered.iter().any(|p| p.name() == "wayland-e2e-fixture"),
        "the e2e fixture plugin must be discovered via inventory"
    );

    // 2. Run initialize_all — the plugin registers echo_fixture; the runner
    //    captures it into InitializeOutcome.tools (Task 1.1 + 1.7).
    let mut runner = PluginRunner::new();
    let outcome = runner
        .initialize_all(&discovered)
        .await
        .expect("initialize_all");
    assert!(
        outcome.errors.is_empty(),
        "no plugin errors expected, got: {:?}",
        outcome.errors
    );
    assert!(
        outcome
            .tools
            .iter()
            .any(|t| t.tool.name == "echo_fixture" && t.plugin == "wayland-e2e-fixture"),
        "the captured tool list must include the e2e fixture's echo_fixture"
    );

    // 3. apply_initialize_outcome reifies the captured tool into the live
    //    ToolRegistry via PluginToolAdapter (Task 1.7).
    let mut registry = ToolRegistry::new();
    let _applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    // 4. The tool is now invocable by name — and the plugin closure runs.
    let tool = registry
        .get("echo_fixture")
        .expect("echo_fixture must be present in the live ToolRegistry");
    assert_eq!(tool.name(), "echo_fixture");

    let result = tool
        .execute(serde_json::json!({ "text": "wayland-e2e" }))
        .await;
    assert!(!result.is_error, "tool execution must succeed: {result:?}");
    assert_eq!(
        result.content, "wayland-e2e",
        "the plugin closure ran end-to-end and echoed its input"
    );
}
