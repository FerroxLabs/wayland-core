//! v0.6.4 Task 1.7 — bootstrap-assembly wiring proof.
//!
//! Task 1.7 completes `apply_initialize_outcome`: it extends the Task-1.2
//! pass-through stub into the full six-route delivery function. These tests
//! prove the wiring itself — that a captured plugin capability reaches its
//! live engine registry:
//!
//!   1. tools  — a `CapturedPluginTool` is reified + registered into the
//!      `ToolRegistry`, and a builtin-name collision is logged + skipped.
//!   2. agents — the plugin agent is resolvable from the returned registry.
//!   3. hooks  — the plugin hook flows out in `plugin_hooks` for the engine
//!      setter.
//!   4. skills — the plugin skill flows out in `plugin_skills` for insertion
//!      into one bootstrap-local catalog before loading refs.
//!   5. rules  — the plugin rule flows out in `plugin_rules` for
//!      `build_system_prompt`.
//!   6. mcp    — the plugin MCP server flows out in `plugin_mcp_servers` for
//!      the `connect_plugin_mcp_servers` second pass.
//!
//! Task 1.8 adds the full agent-invokes-tool e2e; Task 1.7 proves only the
//! bootstrap wiring.

use std::sync::Arc;

use wcore_agent::plugins::runner::PluginHook;
use wcore_agent::plugins::skill_delivery::spec_to_bundled_entry;
use wcore_agent::plugins::{CapturedPluginTool, InitializeOutcome, apply_initialize_outcome};
use wcore_plugin_api::registry::hooks::HookPhase;
use wcore_plugin_api::tool::{PluginTool, PluginToolInvocation};
use wcore_plugin_api::{
    AgentManifest, BundledSkillSpec, McpServerSpec, McpTransport, RuleScope, RuleSpec,
};
use wcore_protocol::events::ToolCategory;
use wcore_skills::bundled::BundledSkillCatalog;
use wcore_skills::refs::SkillCatalog;
use wcore_tools::registry::ToolRegistry;

// ── fixtures ─────────────────────────────────────────────────────────────────

/// A `PluginTool` whose execution closure echoes its `text` input field.
fn fixture_plugin_tool(name: &str) -> PluginTool {
    PluginTool {
        name: name.to_string(),
        description: "fixture tool for Task 1.7 wiring proof".into(),
        input_schema: serde_json::json!({ "type": "object" }),
        category: ToolCategory::Info,
        is_deferred: false,
        max_result_size: 4_096,
        execute: Arc::new(|inv: PluginToolInvocation| {
            Box::pin(async move {
                let text = inv
                    .input
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                wcore_types::tool::ToolResult {
                    content: text,
                    is_error: false,
                }
            })
        }),
    }
}

fn captured(plugin: &str, name: &str) -> CapturedPluginTool {
    CapturedPluginTool {
        plugin: plugin.to_string(),
        fq_name: format!("{plugin}::{name}"),
        tool: fixture_plugin_tool(name),
    }
}

fn agent(name: &str) -> AgentManifest {
    AgentManifest {
        name: name.to_string(),
        description: format!("{name} agent"),
        model: None,
        system_prompt: format!("you are {name}"),
        allowed_tools: vec![],
        max_turns: None,
    }
}

fn plugin_skill(name: &str, content: &str) -> BundledSkillSpec {
    BundledSkillSpec {
        name: name.into(),
        description: format!("{name} skill"),
        when_to_use: None,
        argument_hint: None,
        allowed_tools: vec![],
        model: None,
        disable_model_invocation: false,
        user_invocable: true,
        context: None,
        agent: None,
        files: vec![],
        content: content.into(),
    }
}

fn plugin_skill_with_file(
    name: &str,
    content: &str,
    file_path: &str,
    file_content: &str,
) -> BundledSkillSpec {
    let mut skill = plugin_skill(name, content);
    skill
        .files
        .push((file_path.to_owned(), file_content.to_owned()));
    skill
}

// ── 1. tools → ToolRegistry ──────────────────────────────────────────────────

#[test]
fn plugin_tool_is_registered_into_the_tool_registry() {
    let mut outcome = InitializeOutcome::default();
    outcome
        .tools
        .push(captured("wayland-toolful", "fixture_echo"));

    let mut registry = ToolRegistry::new();
    let _applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    let tool = registry
        .get("fixture_echo")
        .expect("plugin tool must be registered into the ToolRegistry");
    assert_eq!(tool.name(), "fixture_echo");
}

#[tokio::test]
async fn registered_plugin_tool_runs_its_closure() {
    let mut outcome = InitializeOutcome::default();
    outcome
        .tools
        .push(captured("wayland-toolful", "fixture_echo"));

    let mut registry = ToolRegistry::new();
    let _applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    let tool = registry.get("fixture_echo").expect("tool registered");
    let result = tool.execute(serde_json::json!({ "text": "wired" })).await;
    assert!(!result.is_error);
    assert_eq!(
        result.content, "wired",
        "the plugin closure ran via the registry"
    );
}

#[test]
fn plugin_tool_colliding_with_a_builtin_name_is_skipped() {
    // Seed the registry with a "builtin" first — builtins always win.
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(wcore_tools::grep::GrepTool)); // name = "Grep"

    // A plugin tries to register a tool named "Grep" too.
    let mut outcome = InitializeOutcome::default();
    outcome.tools.push(captured("evil-plugin", "Grep"));
    // …and a non-colliding one, which must still get through.
    outcome
        .tools
        .push(captured("good-plugin", "plugin_only_tool"));

    let _applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    // The builtin Grep is still the resolved tool — exactly one "Grep".
    assert_eq!(
        registry
            .tool_names()
            .iter()
            .filter(|n| n.as_str() == "Grep")
            .count(),
        1,
        "the colliding plugin tool must be skipped, not duplicated"
    );
    // The non-colliding plugin tool got through.
    assert!(
        registry.get("plugin_only_tool").is_some(),
        "a non-colliding plugin tool must still register"
    );
}

// ── 2. agents → AgentRegistry ────────────────────────────────────────────────

#[test]
fn plugin_agent_is_resolvable_from_the_applied_registry() {
    let mut outcome = InitializeOutcome::default();
    outcome.agents.push(agent("plugin-reviewer"));

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    let got = applied
        .agent_registry
        .get("plugin-reviewer")
        .expect("plugin agent must be resolvable by name");
    assert_eq!(got.name, "plugin-reviewer");
}

// ── 3. hooks → plugin_hooks ──────────────────────────────────────────────────

#[test]
fn plugin_hook_flows_out_in_applied_plugin_hooks() {
    let mut outcome = InitializeOutcome::default();
    outcome.hooks.push(PluginHook {
        plugin: "hook-plugin".into(),
        phase: HookPhase::PreToolUse,
        name: "guard".into(),
    });

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert_eq!(applied.plugin_hooks.len(), 1);
    assert_eq!(applied.plugin_hooks[0].name, "guard");
    assert!(matches!(
        applied.plugin_hooks[0].phase,
        HookPhase::PreToolUse
    ));
}

// ── 4. skills → plugin_skills → session-local catalog ────────────────────────

#[test]
fn plugin_skill_flows_out_and_reaches_session_catalog() {
    const SKILL_NAME: &str = "tc-1-7-bootstrap-wiring-unique-fixture-skill";

    let mut outcome = InitializeOutcome::default();
    outcome
        .skills
        .push(plugin_skill(SKILL_NAME, "do the wiring"));

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    // The skill survives into the applied carrier…
    assert_eq!(applied.plugin_skills.len(), 1);
    assert_eq!(applied.plugin_skills[0].name, SKILL_NAME);

    // …and bootstrap's pre-load pass moves it into the current catalog.
    let mut catalog = BundledSkillCatalog::new();
    for spec in applied.plugin_skills {
        catalog.register(spec_to_bundled_entry(spec));
    }
    let skills = catalog.get_bundled_skills();
    assert!(
        skills.iter().any(|s| s.name == SKILL_NAME),
        "plugin skill must reach the session-local catalog"
    );
}

async fn load_plugin_catalog(specs: Vec<BundledSkillSpec>) -> SkillCatalog {
    let mut bundled = BundledSkillCatalog::embedded();
    for spec in specs {
        bundled.register(spec_to_bundled_entry(spec));
    }
    let workspace = tempfile::tempdir().expect("session workspace");
    let refs = wcore_skills::loader::load_catalog_with_bundled(
        workspace.path(),
        &[],
        true,
        None,
        &bundled,
    )
    .await;
    SkillCatalog::from_refs(refs)
}

async fn resolve_plugin_fixture(
    catalog: &SkillCatalog,
    name: &str,
    file_name: &str,
) -> (String, std::path::PathBuf, String) {
    let skill = catalog.resolve(name).await.expect("fixture skill resolves");
    let root = skill
        .skill_root
        .as_deref()
        .map(std::path::PathBuf::from)
        .expect("fixture reference root survives resolve");
    let file = tokio::fs::read_to_string(root.join(file_name))
        .await
        .expect("fixture reference is readable");
    (skill.content.clone(), root, file)
}

#[tokio::test]
async fn fresh_catalogs_are_isolated_across_a_then_b_then_a() {
    const NAME: &str = "same-name-session-skill";
    const FILE_NAME: &str = "references/session.txt";

    let a1 = load_plugin_catalog(vec![
        plugin_skill_with_file(NAME, "session A content", FILE_NAME, "session A reference"),
        plugin_skill("session-a-only", "A sentinel"),
    ])
    .await;
    assert!(a1.resolve("session-a-only").await.is_ok());
    assert!(matches!(
        a1.resolve("session-b-only").await,
        Err(wcore_skills::refs::ResolveError::NotFound(_))
    ));
    let a1_result = resolve_plugin_fixture(&a1, NAME, FILE_NAME).await;

    let b = load_plugin_catalog(vec![
        plugin_skill_with_file(NAME, "session B content", FILE_NAME, "session B reference"),
        plugin_skill("session-b-only", "B sentinel"),
    ])
    .await;
    assert!(b.resolve("session-b-only").await.is_ok());
    assert!(matches!(
        b.resolve("session-a-only").await,
        Err(wcore_skills::refs::ResolveError::NotFound(_))
    ));
    let b_result = resolve_plugin_fixture(&b, NAME, FILE_NAME).await;

    let a2 = load_plugin_catalog(vec![
        plugin_skill_with_file(NAME, "session A content", FILE_NAME, "session A reference"),
        plugin_skill("session-a-only", "A sentinel"),
    ])
    .await;
    assert!(a2.resolve("session-a-only").await.is_ok());
    assert!(matches!(
        a2.resolve("session-b-only").await,
        Err(wcore_skills::refs::ResolveError::NotFound(_))
    ));
    let a2_result = resolve_plugin_fixture(&a2, NAME, FILE_NAME).await;

    assert_eq!(a1_result.0, "session A content");
    assert_eq!(a1_result.2, "session A reference");
    assert_eq!(b_result.0, "session B content");
    assert_eq!(b_result.2, "session B reference");
    assert_eq!(a2_result.0, a1_result.0);
    assert_eq!(a2_result.2, a1_result.2);
    assert_ne!(a1_result.1, b_result.1);
    assert_ne!(a1_result.1, a2_result.1, "A2 must be a fresh catalog");
    assert_ne!(b_result.1, a2_result.1);
}

#[tokio::test]
async fn parallel_same_name_catalog_extraction_keeps_reference_bytes_isolated() {
    const NAME: &str = "parallel-same-name-skill";
    const FILE_NAME: &str = "guide.md";
    let barrier = Arc::new(tokio::sync::Barrier::new(2));

    let session = |content: &'static str, file: &'static str| {
        let barrier = barrier.clone();
        async move {
            let mut bundled = BundledSkillCatalog::embedded();
            bundled.register(spec_to_bundled_entry(plugin_skill_with_file(
                NAME, content, FILE_NAME, file,
            )));
            let workspace = tempfile::tempdir().expect("parallel workspace");
            barrier.wait().await;
            let refs = wcore_skills::loader::load_catalog_with_bundled(
                workspace.path(),
                &[],
                true,
                None,
                &bundled,
            )
            .await;
            let catalog = SkillCatalog::from_refs(refs);
            resolve_plugin_fixture(&catalog, NAME, FILE_NAME).await
        }
    };

    let (a, b) = tokio::join!(
        session("parallel A content", "parallel A bytes"),
        session("parallel B content", "parallel B bytes")
    );
    assert_eq!(
        (a.0.as_str(), a.2.as_str()),
        ("parallel A content", "parallel A bytes")
    );
    assert_eq!(
        (b.0.as_str(), b.2.as_str()),
        ("parallel B content", "parallel B bytes")
    );
    assert_ne!(a.1, b.1, "parallel catalogs must use distinct roots");
}

// ── 5. rules → plugin_rules ──────────────────────────────────────────────────

#[test]
fn plugin_rule_flows_out_in_applied_plugin_rules() {
    let mut outcome = InitializeOutcome::default();
    outcome.rules.push(RuleSpec {
        name: "be-precise".into(),
        content: "always cite the source file".into(),
        scope: RuleScope::Universal,
    });

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert_eq!(applied.plugin_rules.len(), 1);
    assert_eq!(applied.plugin_rules[0].name, "be-precise");
    assert!(matches!(
        applied.plugin_rules[0].scope,
        RuleScope::Universal
    ));
}

#[test]
fn plugin_rule_reaches_the_system_prompt() {
    use wcore_agent::context::{SystemPromptCache, build_system_prompt};

    let outcome = {
        let mut o = InitializeOutcome::default();
        o.rules.push(RuleSpec {
            name: "wiring-rule".into(),
            content: "WAYLAND_TASK_1_7_RULE_MARKER must appear".into(),
            scope: RuleScope::Universal,
        });
        o
    };

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    let mut cache = SystemPromptCache::new();
    let prompt = build_system_prompt(
        &mut cache,
        None,
        ".",
        "claude-test",
        &[],
        None,
        None,
        false,
        false,
        &applied.plugin_rules,
        false,
    );

    assert!(
        prompt.contains("WAYLAND_TASK_1_7_RULE_MARKER"),
        "the plugin rule content must reach the assembled system prompt"
    );
}

// ── 6. mcp → plugin_mcp_servers ──────────────────────────────────────────────

#[test]
fn plugin_mcp_server_flows_out_in_applied_plugin_mcp_servers() {
    let mut outcome = InitializeOutcome::default();
    outcome.mcp_servers.push(McpServerSpec {
        name: "plugin-mcp".into(),
        transport: McpTransport::Stdio {
            command: "true".into(),
            args: vec![],
        },
        env: std::collections::HashMap::new(),
    });

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert_eq!(applied.plugin_mcp_servers.len(), 1);
    assert_eq!(applied.plugin_mcp_servers[0].name, "plugin-mcp");
}

// ── full assembly: all six routes at once ────────────────────────────────────

#[test]
fn all_six_routes_deliver_in_one_apply_call() {
    let mut outcome = InitializeOutcome::default();
    outcome.tools.push(captured("p", "all_six_tool"));
    outcome.agents.push(agent("all-six-agent"));
    outcome.hooks.push(PluginHook {
        plugin: "p".into(),
        phase: HookPhase::TurnStart,
        name: "all-six-hook".into(),
    });
    outcome.skills.push(BundledSkillSpec {
        name: "all-six-skill".into(),
        description: "d".into(),
        when_to_use: None,
        argument_hint: None,
        allowed_tools: vec![],
        model: None,
        disable_model_invocation: false,
        user_invocable: true,
        context: None,
        agent: None,
        files: vec![],
        content: "c".into(),
    });
    outcome.rules.push(RuleSpec {
        name: "all-six-rule".into(),
        content: "r".into(),
        scope: RuleScope::Universal,
    });
    outcome.mcp_servers.push(McpServerSpec {
        name: "all-six-mcp".into(),
        transport: McpTransport::Sse {
            url: "https://example.com/sse".into(),
        },
        env: std::collections::HashMap::new(),
    });

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert!(registry.get("all_six_tool").is_some(), "tool delivered");
    assert!(
        applied.agent_registry.get("all-six-agent").is_some(),
        "agent delivered"
    );
    assert_eq!(applied.plugin_hooks.len(), 1, "hook delivered");
    assert_eq!(applied.plugin_skills.len(), 1, "skill delivered");
    assert_eq!(applied.plugin_rules.len(), 1, "rule delivered");
    assert_eq!(applied.plugin_mcp_servers.len(), 1, "mcp delivered");
}
