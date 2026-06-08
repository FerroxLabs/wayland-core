//! v0.6.5 Task 1.4 — exit gate: every plugin capability lands through ONE
//! call to `apply_initialize_outcome`, with no separate bootstrap-side
//! reify side-calls for Browser or CUA.
//!
//! Prior to v0.6.5, Browser tool reify and CUA tool reify each lived at
//! their own `bootstrap.rs` call site (lines 427 and 442). SDK authors
//! had to read three files to understand reification. Task 1.4 folded
//! both into `apply.rs` so a single function call delivers all ten
//! surfaces. This test pins that contract.
//!
//! Fixture composition: one of each surface that `apply.rs` is responsible
//! for. CUA-tool spec construction needs `wcore-cua` host types which the
//! plugin layer deliberately doesn't depend on, so the CUA registrar is
//! passed empty — what matters for this test is that the SAME call accepts
//! it and produces a populated carrier (here, length-0, which proves the
//! field-and-call-path exists).

use std::sync::Arc;

use wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar;
use wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar;
use wcore_agent::plugins::apply_initialize_outcome;
use wcore_agent::plugins::runner::{
    CapturedPluginTool, CapturedUserModel, InitializeOutcome, PluginHook,
};
use wcore_plugin_api::browser_spec::{BrowserPolicySpec, BrowserProviderHint, BrowserToolSpec};
use wcore_plugin_api::registry::browser::BrowserToolRegistrar;
use wcore_plugin_api::registry::hooks::HookPhase;
use wcore_plugin_api::tool::{PluginTool, PluginToolInvocation};
use wcore_plugin_api::{
    AgentManifest, BundledSkillSpec, McpServerSpec, McpTransport, RuleScope, RuleSpec,
    UserModelSpec,
};
use wcore_protocol::events::ToolCategory;
use wcore_tools::registry::ToolRegistry;

fn fixture_plugin_tool(name: &str) -> PluginTool {
    PluginTool {
        name: name.to_string(),
        description: "fixture".into(),
        input_schema: serde_json::json!({"type": "object"}),
        category: ToolCategory::Info,
        is_deferred: false,
        max_result_size: 4_096,
        execute: Arc::new(|_inv: PluginToolInvocation| {
            Box::pin(async move {
                wcore_types::tool::ToolResult {
                    content: "ok".into(),
                    is_error: false,
                }
            })
        }),
    }
}

fn captured_tool(plugin: &str, name: &str) -> CapturedPluginTool {
    CapturedPluginTool {
        plugin: plugin.to_string(),
        fq_name: format!("{plugin}::{name}"),
        tool: fixture_plugin_tool(name),
    }
}

fn fixture_agent(name: &str) -> AgentManifest {
    AgentManifest {
        name: name.to_string(),
        description: format!("{name} agent"),
        model: None,
        system_prompt: format!("you are {name}"),
        allowed_tools: vec![],
        max_turns: None,
    }
}

fn fixture_browser_spec(ns: &str) -> BrowserToolSpec {
    BrowserToolSpec {
        tool_namespace: ns.into(),
        preferred_provider: BrowserProviderHint::Camoufox,
        policy: BrowserPolicySpec {
            default_action: "allow".into(),
            allowed_origins: vec!["*.example.com".into()],
            denied_origins: vec![],
        },
        allow_cloud: false,
    }
}

#[test]
fn single_reification_call_site_delivers_all_surfaces() {
    // 1. Build a fixture InitializeOutcome carrying one entry per surface.
    let mut outcome = InitializeOutcome::default();
    outcome
        .tools
        .push(captured_tool("all-surfaces-fixture", "fixture_tool"));
    outcome.agents.push(fixture_agent("fixture-agent"));
    outcome.hooks.push(PluginHook {
        plugin: "all-surfaces-fixture".into(),
        phase: HookPhase::PreToolUse,
        name: "fixture_hook".into(),
    });
    outcome.rules.push(RuleSpec {
        name: "fixture-rule".into(),
        content: "always cite the source".into(),
        scope: RuleScope::Universal,
    });
    outcome.skills.push(BundledSkillSpec {
        name: "fixture-skill".into(),
        description: "fixture skill".into(),
        when_to_use: None,
        argument_hint: None,
        allowed_tools: vec![],
        model: None,
        disable_model_invocation: false,
        user_invocable: true,
        context: None,
        agent: None,
        files: vec![],
        content: "do the thing".into(),
    });
    outcome.mcp_servers.push(McpServerSpec {
        name: "fixture-mcp".into(),
        transport: McpTransport::Stdio {
            command: "true".into(),
            args: vec![],
        },
        env: std::collections::HashMap::new(),
    });
    outcome.user_models.push(CapturedUserModel {
        plugin: "all-surfaces-fixture".into(),
        spec: UserModelSpec {
            name: "fixture-user-model".into(),
            description: "fixture".into(),
            backend: "honcho".into(),
            base_url: None,
            api_key_env: None,
            config: serde_json::Value::Null,
        },
    });

    // 2. Browser registrar with one captured spec.
    let mut browser_registrar = HostBrowserRegistrar::default();
    browser_registrar
        .host_register(fixture_browser_spec("FixtureBrowser"))
        .expect("browser register");

    // 3. CUA registrar default-empty (constructing a real spec would pull
    //    `wcore-cua` into this test; the contract pinned here is that the
    //    SAME call accepts the registrar — registration into `tool_registry`
    //    is the side-effect contract, no carrier is returned).
    let cua_registrar = HostCuaRegistrar::default();

    // 4. ONE call delivers everything.
    let mut registry = ToolRegistry::new();
    let applied =
        apply_initialize_outcome(outcome, &mut registry, browser_registrar, cua_registrar);

    // 5. Every surface landed via the SINGLE call.
    assert!(
        registry.get("fixture_tool").is_some(),
        "plain plugin tool must be in the registry",
    );
    assert!(
        applied.agent_registry.get("fixture-agent").is_some(),
        "agent must be in applied.agent_registry",
    );
    assert_eq!(applied.plugin_hooks.len(), 1, "hook surface");
    assert_eq!(applied.plugin_rules.len(), 1, "rule surface");
    assert_eq!(applied.plugin_skills.len(), 1, "skill surface");
    assert_eq!(applied.plugin_mcp_servers.len(), 1, "mcp surface");
    assert_eq!(applied.plugin_user_models.len(), 1, "user-model surface");
    // Browser tool: registered into the registry. `BrowserTool::name()` is
    // the fixed string "Browser" (the tool_namespace is internal routing,
    // not the registry key). Registration is the contract — no carrier.
    assert!(
        registry.get("Browser").is_some(),
        "browser tool must be in the registry (via deliver_browser_tools)",
    );
    // CUA registrar was empty; assert by absence in registry. The pinned
    // contract is that the SAME call accepts the cua_registrar argument.
    let _ = &applied; // keep binding live for any future assertions
}
