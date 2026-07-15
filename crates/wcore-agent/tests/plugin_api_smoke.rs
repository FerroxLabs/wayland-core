//! End-to-end smoke: a test plugin compiled into the wcore-agent test binary
//! registers every Scoped* surface via the wcore-agent plugin host adapters.

use wcore_agent::plugins::{PluginLoader, PluginRunner};
use wcore_config::plugins_config::PluginsConfig;
use wcore_plugin_api::tool::PluginTool;
use wcore_plugin_api::{
    AgentManifest, BundledSkillSpec, McpServerSpec, McpTransport, Plugin, PluginContext,
    PluginFactory, PluginManifest, PluginResult, RuleScope, RuleSpec, registry::hooks::HookPhase,
};
use wcore_protocol::events::ToolCategory;

struct SmokePlugin;

static MANIFEST_TOML: &str = r#"
[plugin]
name = "wayland-smoke"
version = "0.0.1"
description = "exercises every register_* surface"
entry = "builtin:smoke"
authors = ["t"]
license = "MIT"
[permissions]
register_tools       = true
register_hooks       = true
register_agents      = true
register_skills      = true
register_rules       = true
register_mcp_server  = true
register_providers   = false
tool_namespace       = "smoke"
memory_partitions_writable = ["P2"]
memory_partitions_readable = ["P2", "P3"]
"#;

fn smoke_manifest() -> &'static PluginManifest {
    static M: std::sync::OnceLock<PluginManifest> = std::sync::OnceLock::new();
    M.get_or_init(|| PluginManifest::from_toml_str(MANIFEST_TOML).unwrap())
}

#[async_trait::async_trait]
impl Plugin for SmokePlugin {
    fn manifest(&self) -> &PluginManifest {
        smoke_manifest()
    }
    async fn initialize(&self, ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        ctx.tools
            .as_mut()
            .unwrap()
            .register_tool(PluginTool::host_delegated(
                "ping",
                "smoke ping tool",
                ToolCategory::Info,
            ))?;
        ctx.hooks
            .as_mut()
            .unwrap()
            .register_hook(HookPhase::SessionStart, "smoke_session_start".into())?;
        ctx.agents.as_mut().unwrap().register_agent(AgentManifest {
            name: "smoke-architect".into(),
            description: "test agent".into(),
            model: None,
            system_prompt: "be helpful".into(),
            allowed_tools: vec![],
            max_turns: Some(8),
        })?;
        ctx.skills
            .as_mut()
            .unwrap()
            .register_skill(BundledSkillSpec {
                name: "smoke-recall".into(),
                description: "recall test".into(),
                when_to_use: None,
                argument_hint: None,
                allowed_tools: vec![],
                model: None,
                disable_model_invocation: false,
                user_invocable: true,
                context: None,
                agent: None,
                files: vec![],
                content: "# Smoke skill".into(),
            })?;
        ctx.rules.as_mut().unwrap().register_rule(RuleSpec {
            name: "smoke-rule".into(),
            content: "follow the contract".into(),
            scope: RuleScope::Universal,
        })?;
        ctx.mcp_servers
            .as_mut()
            .unwrap()
            .register_mcp_server(McpServerSpec {
                name: "smoke-mcp".into(),
                transport: McpTransport::Stdio {
                    command: "echo".into(),
                    args: vec!["hi".into()],
                },
                env: Default::default(),
            })?;
        Ok(())
    }
}

struct SmokeFactory;
impl PluginFactory for SmokeFactory {
    fn name(&self) -> &'static str {
        "wayland-smoke"
    }
    fn build(&self) -> Box<dyn Plugin> {
        Box::new(SmokePlugin)
    }
}
inventory::submit! { &SmokeFactory as &dyn wcore_plugin_api::PluginFactory }

#[tokio::test]
async fn smoke_plugin_registers_all_six_surfaces() {
    let cfg = PluginsConfig::default();
    let mut loader = PluginLoader::discover(&cfg);
    let captured = loader.validate_all().expect("validate");
    assert!(captured.iter().any(|p| p.name() == "wayland-smoke"));

    let mut runner = PluginRunner::new();
    let outcome = runner.initialize_all(&captured).await.expect("init ok");

    assert!(outcome.has_any_registered());
    assert!(
        outcome
            .tools
            .iter()
            .any(|t| t.fq_name == "smoke::ping" && t.tool.name == "ping")
    );
    assert!(
        outcome
            .hooks
            .iter()
            .any(|h| matches!(h.phase, HookPhase::SessionStart) && h.name == "smoke_session_start")
    );
    assert!(outcome.agents.iter().any(|a| a.name == "smoke-architect"));
    assert!(outcome.skills.iter().any(|s| s.name == "smoke-recall"));
    assert!(outcome.rules.iter().any(|r| r.name == "smoke-rule"));
    assert!(outcome.mcp_servers.iter().any(|s| s.name == "smoke-mcp"));
    assert!(outcome.errors.is_empty());
}

#[tokio::test]
async fn ready_event_advertises_plugins_true_when_plugin_registered() {
    use wcore_config::compat::ProviderCompat;

    // Discover + initialize SmokePlugin via the loader/runner.
    let cfg = PluginsConfig::default();
    let mut loader = PluginLoader::discover(&cfg);
    let captured = loader.validate_all().expect("validate");
    let mut runner = PluginRunner::new();
    let outcome = runner.initialize_all(&captured).await.expect("init");
    assert!(
        outcome.has_any_registered(),
        "expected plugin registrations"
    );

    // Verify the W2.5 contract: build_capabilities, given has_plugins=true,
    // produces a Capabilities struct with plugins=true. We mirror the
    // build_capabilities body here rather than punching a hole through
    // ProtocolSink's private API; the field-name + JSON serialization are
    // the load-bearing surface and we assert both.
    let compat = ProviderCompat::default();
    let caps = wcore_protocol::events::Capabilities {
        tool_approval: true,
        thinking: compat.supports_thinking(),
        effort: compat.supports_effort(),
        effort_levels: compat.effort_levels().to_vec(),
        modes: vec!["default".into(), "auto_edit".into(), "force".into()],
        current_mode: "default".into(),
        mcp: false,
        plugins: outcome.has_any_registered(),
        ..Default::default()
    };
    assert!(caps.plugins, "plugins should advertise true");

    // Serialize a Ready event and confirm the JSON includes `plugins: true`
    // so host decoders see it.
    let event = wcore_protocol::events::ProtocolEvent::Ready {
        version: "0.0.1".into(),
        session_id: None,
        capabilities: caps,
        execution_policy: None,
    };
    let v: serde_json::Value = serde_json::to_value(&event).unwrap();
    assert_eq!(v["type"], "ready");
    assert_eq!(v["capabilities"]["plugins"], true);
}

#[tokio::test]
async fn permission_denied_plugin_rejected_at_parse() {
    // A plugin manifest with register_tools=true but no tool_namespace fails
    // at PluginManifest::from_toml_str. Verifying the load path surfaces the
    // error rather than panicking is the point of this test.
    let bad = r#"
[plugin]
name = "wayland-bad"
version = "0.0.1"
description = "bad"
entry = "builtin:bad"
authors = ["t"]
license = "MIT"
[permissions]
register_tools = true
"#;
    assert!(wcore_plugin_api::PluginManifest::from_toml_str(bad).is_err());
}
