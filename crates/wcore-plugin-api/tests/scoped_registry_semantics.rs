//! Tests for `Scoped*Registry` namespace enforcement, permission gates, and
//! `NamespaceLedger` cross-plugin collision detection.

use std::sync::Arc;

use wcore_plugin_api::registry::agents::{AgentRegistrar, ScopedAgentRegistry};
use wcore_plugin_api::registry::hooks::{HookPhase, HookRegistrar, ScopedHookRegistry};
use wcore_plugin_api::registry::mcp::{McpRegistrar, ScopedMcpRegistry};
use wcore_plugin_api::registry::providers::{
    PluginProvider, ProviderRegistrar, ScopedProviderRegistry,
};
use wcore_plugin_api::registry::rules::{RuleRegistrar, ScopedRuleRegistry};
use wcore_plugin_api::registry::skills::{ScopedSkillRegistry, SkillRegistrar};
use wcore_plugin_api::registry::tools::{NamespaceLedger, ScopedToolRegistry, ToolRegistrar};
use wcore_plugin_api::tool::PluginTool;
use wcore_plugin_api::{
    AgentManifest, BundledSkillSpec, McpServerSpec, McpTransport, PluginError, PluginManifest,
    RuleScope, RuleSpec,
};
use wcore_protocol::events::ToolCategory;

/// Construct a minimal `PluginTool` for tests — a host-delegated tool
/// carries honest metadata and an error closure.
fn test_tool(name: &str) -> PluginTool {
    PluginTool::host_delegated(name, "test tool", ToolCategory::Info)
}

// ---------------------------------------------------------------------------
// ScopedToolRegistry
// ---------------------------------------------------------------------------

struct CaptureRegistrar {
    registered: Vec<String>,
}

impl ToolRegistrar for CaptureRegistrar {
    fn host_register(
        &mut self,
        fully_qualified_name: String,
        _tool: PluginTool,
    ) -> Result<(), String> {
        if self.registered.contains(&fully_qualified_name) {
            return Err(format!("duplicate: {fully_qualified_name}"));
        }
        self.registered.push(fully_qualified_name);
        Ok(())
    }
}

fn ijfw_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-ijfw"
version = "1.0.0"
description = "test"
entry = "builtin:ijfw"
authors = ["t"]
license = "MIT"
[permissions]
register_tools = true
tool_namespace = "ijfw"
"#,
    )
    .expect("ijfw test manifest")
}

fn empty_manifest(name: &str) -> PluginManifest {
    PluginManifest::from_toml_str(&format!(
        r#"
[plugin]
name = "{name}"
version = "1.0.0"
description = "t"
entry = "builtin:t"
authors = ["t"]
license = "MIT"
"#,
    ))
    .unwrap()
}

#[test]
fn scoped_tool_register_prefixes_with_namespace() {
    let m = ijfw_manifest();
    let mut host = CaptureRegistrar {
        registered: Vec::new(),
    };
    let mut scoped = ScopedToolRegistry::new(&m, &mut host).expect("scoped reg");
    scoped
        .register_tool(test_tool("memory_search"))
        .expect("registered");
    assert_eq!(host.registered, vec!["ijfw::memory_search".to_string()]);
}

#[test]
fn scoped_tool_rejects_name_already_namespaced() {
    let m = ijfw_manifest();
    let mut host = CaptureRegistrar {
        registered: Vec::new(),
    };
    let mut scoped = ScopedToolRegistry::new(&m, &mut host).expect("scoped reg");
    let err = scoped
        .register_tool(test_tool("browser::click"))
        .expect_err("must reject");
    assert!(
        matches!(err, PluginError::ToolNameOutsideNamespace { .. }),
        "expected ToolNameOutsideNamespace, got {err:?}"
    );
}

#[test]
fn scoped_tool_registry_cannot_be_constructed_without_permission() {
    let m_no_tools = empty_manifest("wayland-readonly");
    let mut host = CaptureRegistrar {
        registered: Vec::new(),
    };
    let result = ScopedToolRegistry::new(&m_no_tools, &mut host);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("must reject"),
    };
    assert!(matches!(err, PluginError::PermissionDenied { .. }));
}

#[test]
fn scoped_tool_registry_rejects_duplicate() {
    let m = ijfw_manifest();
    let mut host = CaptureRegistrar {
        registered: Vec::new(),
    };
    let mut scoped = ScopedToolRegistry::new(&m, &mut host).expect("scoped reg");
    scoped
        .register_tool(test_tool("memory_search"))
        .expect("first");
    let err = scoped
        .register_tool(test_tool("memory_search"))
        .expect_err("dup");
    assert!(matches!(err, PluginError::DuplicateRegistration { .. }));
}

// ---------------------------------------------------------------------------
// NamespaceLedger
// ---------------------------------------------------------------------------

#[test]
fn namespace_ledger_rejects_second_claim() {
    let mut ledger = NamespaceLedger::default();
    ledger
        .claim("Browser", "wayland-browser")
        .expect("first claim");
    let err = ledger
        .claim("Browser", "wayland-browser-fork")
        .expect_err("second claim must fail");
    assert!(matches!(err, PluginError::NamespaceCollision { .. }));
}

#[test]
fn namespace_ledger_allows_same_plugin_repeated() {
    let mut ledger = NamespaceLedger::default();
    ledger.claim("ijfw", "wayland-ijfw").unwrap();
    ledger.claim("ijfw", "wayland-ijfw").unwrap();
}

// ---------------------------------------------------------------------------
// ScopedHookRegistry
// ---------------------------------------------------------------------------

struct CaptureHooks {
    registered: Vec<(HookPhase, String)>,
}

impl HookRegistrar for CaptureHooks {
    fn host_register_hook(&mut self, phase: HookPhase, name: String) -> Result<(), String> {
        self.registered.push((phase, name));
        Ok(())
    }
}

fn hooks_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-hooked"
version = "1.0.0"
description = "t"
entry = "builtin:h"
authors = ["t"]
license = "MIT"
[permissions]
register_hooks = true
"#,
    )
    .unwrap()
}

#[test]
fn scoped_hooks_with_permission_registers() {
    let m = hooks_manifest();
    let mut host = CaptureHooks {
        registered: Vec::new(),
    };
    let mut scoped = ScopedHookRegistry::new(&m, &mut host).unwrap();
    scoped
        .register_hook(HookPhase::SessionStart, "ijfw_session_start".into())
        .unwrap();
    assert_eq!(
        host.registered,
        vec![(HookPhase::SessionStart, "ijfw_session_start".into())]
    );
}

#[test]
fn scoped_hooks_constructor_rejects_without_permission() {
    let m_no_hooks = empty_manifest("wayland-mute");
    let mut host = CaptureHooks {
        registered: Vec::new(),
    };
    let result = ScopedHookRegistry::new(&m_no_hooks, &mut host);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("no perm"),
    };
    assert!(matches!(err, PluginError::PermissionDenied { .. }));
}

#[test]
fn scoped_hooks_rejects_duplicate() {
    let m = hooks_manifest();
    let mut host = CaptureHooks {
        registered: Vec::new(),
    };
    let mut scoped = ScopedHookRegistry::new(&m, &mut host).unwrap();
    scoped
        .register_hook(HookPhase::SessionStart, "h1".into())
        .unwrap();
    let err = scoped
        .register_hook(HookPhase::SessionStart, "h1".into())
        .expect_err("dup");
    assert!(matches!(err, PluginError::DuplicateRegistration { .. }));
}

// ---------------------------------------------------------------------------
// ScopedAgentRegistry
// ---------------------------------------------------------------------------

struct CaptureAgents {
    registered: Vec<AgentManifest>,
}

impl AgentRegistrar for CaptureAgents {
    fn host_register_agent(&mut self, agent: AgentManifest) -> Result<(), String> {
        self.registered.push(agent);
        Ok(())
    }
}

fn agents_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-agentful"
version = "1.0.0"
description = "t"
entry = "builtin:a"
authors = ["t"]
license = "MIT"
[permissions]
register_agents = true
"#,
    )
    .unwrap()
}

fn sample_agent() -> AgentManifest {
    AgentManifest {
        name: "researcher".into(),
        description: "d".into(),
        model: None,
        system_prompt: "p".into(),
        allowed_tools: vec![],
        max_turns: None,
    }
}

#[test]
fn scoped_agents_with_permission_registers() {
    let m = agents_manifest();
    let mut host = CaptureAgents {
        registered: Vec::new(),
    };
    let mut scoped = ScopedAgentRegistry::new(&m, &mut host).unwrap();
    scoped.register_agent(sample_agent()).unwrap();
    assert_eq!(host.registered.len(), 1);
}

#[test]
fn scoped_agents_constructor_rejects_without_permission() {
    let m_no = empty_manifest("wayland-no-agents");
    let mut host = CaptureAgents {
        registered: Vec::new(),
    };
    let result = ScopedAgentRegistry::new(&m_no, &mut host);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("no perm"),
    };
    assert!(matches!(err, PluginError::PermissionDenied { .. }));
}

#[test]
fn scoped_agents_rejects_duplicate() {
    let m = agents_manifest();
    let mut host = CaptureAgents {
        registered: Vec::new(),
    };
    let mut scoped = ScopedAgentRegistry::new(&m, &mut host).unwrap();
    scoped.register_agent(sample_agent()).unwrap();
    let err = scoped.register_agent(sample_agent()).expect_err("dup");
    assert!(matches!(err, PluginError::DuplicateRegistration { .. }));
}

// ---------------------------------------------------------------------------
// ScopedSkillRegistry
// ---------------------------------------------------------------------------

struct CaptureSkills {
    registered: Vec<BundledSkillSpec>,
}

impl SkillRegistrar for CaptureSkills {
    fn host_register_skill(&mut self, skill: BundledSkillSpec) -> Result<(), String> {
        self.registered.push(skill);
        Ok(())
    }
}

fn skills_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-skilled"
version = "1.0.0"
description = "t"
entry = "builtin:s"
authors = ["t"]
license = "MIT"
[permissions]
register_skills = true
"#,
    )
    .unwrap()
}

fn sample_skill() -> BundledSkillSpec {
    BundledSkillSpec {
        name: "recall".into(),
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
        content: "# Recall".into(),
    }
}

#[test]
fn scoped_skills_with_permission_registers() {
    let m = skills_manifest();
    let mut host = CaptureSkills {
        registered: Vec::new(),
    };
    let mut scoped = ScopedSkillRegistry::new(&m, &mut host).unwrap();
    scoped.register_skill(sample_skill()).unwrap();
    assert_eq!(host.registered.len(), 1);
}

#[test]
fn scoped_skills_constructor_rejects_without_permission() {
    let m_no = empty_manifest("wayland-no-skills");
    let mut host = CaptureSkills {
        registered: Vec::new(),
    };
    let result = ScopedSkillRegistry::new(&m_no, &mut host);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("no perm"),
    };
    assert!(matches!(err, PluginError::PermissionDenied { .. }));
}

#[test]
fn scoped_skills_rejects_duplicate() {
    let m = skills_manifest();
    let mut host = CaptureSkills {
        registered: Vec::new(),
    };
    let mut scoped = ScopedSkillRegistry::new(&m, &mut host).unwrap();
    scoped.register_skill(sample_skill()).unwrap();
    let err = scoped.register_skill(sample_skill()).expect_err("dup");
    assert!(matches!(err, PluginError::DuplicateRegistration { .. }));
}

// ---------------------------------------------------------------------------
// ScopedRuleRegistry
// ---------------------------------------------------------------------------

struct CaptureRules {
    registered: Vec<RuleSpec>,
}

impl RuleRegistrar for CaptureRules {
    fn host_register_rule(&mut self, rule: RuleSpec) -> Result<(), String> {
        self.registered.push(rule);
        Ok(())
    }
}

fn rules_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-ruled"
version = "1.0.0"
description = "t"
entry = "builtin:r"
authors = ["t"]
license = "MIT"
[permissions]
register_rules = true
"#,
    )
    .unwrap()
}

fn sample_rule() -> RuleSpec {
    RuleSpec {
        name: "no-emoji".into(),
        content: "do not use emoji".into(),
        scope: RuleScope::Universal,
    }
}

#[test]
fn scoped_rules_with_permission_registers() {
    let m = rules_manifest();
    let mut host = CaptureRules {
        registered: Vec::new(),
    };
    let mut scoped = ScopedRuleRegistry::new(&m, &mut host).unwrap();
    scoped.register_rule(sample_rule()).unwrap();
    assert_eq!(host.registered.len(), 1);
}

#[test]
fn scoped_rules_constructor_rejects_without_permission() {
    let m_no = empty_manifest("wayland-no-rules");
    let mut host = CaptureRules {
        registered: Vec::new(),
    };
    let result = ScopedRuleRegistry::new(&m_no, &mut host);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("no perm"),
    };
    assert!(matches!(err, PluginError::PermissionDenied { .. }));
}

#[test]
fn scoped_rules_rejects_duplicate() {
    let m = rules_manifest();
    let mut host = CaptureRules {
        registered: Vec::new(),
    };
    let mut scoped = ScopedRuleRegistry::new(&m, &mut host).unwrap();
    scoped.register_rule(sample_rule()).unwrap();
    let err = scoped.register_rule(sample_rule()).expect_err("dup");
    assert!(matches!(err, PluginError::DuplicateRegistration { .. }));
}

// ---------------------------------------------------------------------------
// ScopedMcpRegistry
// ---------------------------------------------------------------------------

struct CaptureMcp {
    registered: Vec<McpServerSpec>,
}

impl McpRegistrar for CaptureMcp {
    fn host_register_mcp_server(&mut self, server: McpServerSpec) -> Result<(), String> {
        self.registered.push(server);
        Ok(())
    }
}

fn mcp_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-mcper"
version = "1.0.0"
description = "t"
entry = "builtin:m"
authors = ["t"]
license = "MIT"
[permissions]
register_mcp_server = true
"#,
    )
    .unwrap()
}

fn sample_mcp() -> McpServerSpec {
    McpServerSpec {
        name: "ext".into(),
        transport: McpTransport::Stdio {
            command: "echo".into(),
            args: vec![],
        },
        env: Default::default(),
    }
}

#[test]
fn scoped_mcp_with_permission_registers() {
    let m = mcp_manifest();
    let mut host = CaptureMcp {
        registered: Vec::new(),
    };
    let mut scoped = ScopedMcpRegistry::new(&m, &mut host).unwrap();
    scoped.register_mcp_server(sample_mcp()).unwrap();
    assert_eq!(host.registered.len(), 1);
}

#[test]
fn scoped_mcp_constructor_rejects_without_permission() {
    let m_no = empty_manifest("wayland-no-mcp");
    let mut host = CaptureMcp {
        registered: Vec::new(),
    };
    let result = ScopedMcpRegistry::new(&m_no, &mut host);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("no perm"),
    };
    assert!(matches!(err, PluginError::PermissionDenied { .. }));
}

#[test]
fn scoped_mcp_rejects_duplicate() {
    let m = mcp_manifest();
    let mut host = CaptureMcp {
        registered: Vec::new(),
    };
    let mut scoped = ScopedMcpRegistry::new(&m, &mut host).unwrap();
    scoped.register_mcp_server(sample_mcp()).unwrap();
    let err = scoped.register_mcp_server(sample_mcp()).expect_err("dup");
    assert!(matches!(err, PluginError::DuplicateRegistration { .. }));
}

// ---------------------------------------------------------------------------
// ScopedProviderRegistry
// ---------------------------------------------------------------------------

struct CaptureProviders {
    registered: Vec<String>,
}

impl ProviderRegistrar for CaptureProviders {
    fn host_register_provider(&mut self, provider: Arc<dyn PluginProvider>) -> Result<(), String> {
        self.registered.push(provider.provider_name().to_string());
        Ok(())
    }
}

struct DummyProvider {
    name: String,
}

impl PluginProvider for DummyProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn providers_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-provider"
version = "1.0.0"
description = "t"
entry = "builtin:p"
authors = ["t"]
license = "MIT"
[permissions]
register_providers = true
"#,
    )
    .unwrap()
}

#[test]
fn scoped_providers_with_permission_registers() {
    let m = providers_manifest();
    let mut host = CaptureProviders {
        registered: Vec::new(),
    };
    let mut scoped = ScopedProviderRegistry::new(&m, &mut host).unwrap();
    scoped
        .register_provider(Arc::new(DummyProvider {
            name: "ollama".into(),
        }))
        .unwrap();
    assert_eq!(host.registered, vec!["ollama".to_string()]);
}

#[test]
fn scoped_providers_constructor_rejects_without_permission() {
    let m_no = empty_manifest("wayland-no-providers");
    let mut host = CaptureProviders {
        registered: Vec::new(),
    };
    let result = ScopedProviderRegistry::new(&m_no, &mut host);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("no perm"),
    };
    assert!(matches!(err, PluginError::PermissionDenied { .. }));
}

#[test]
fn scoped_providers_rejects_duplicate() {
    let m = providers_manifest();
    let mut host = CaptureProviders {
        registered: Vec::new(),
    };
    let mut scoped = ScopedProviderRegistry::new(&m, &mut host).unwrap();
    scoped
        .register_provider(Arc::new(DummyProvider {
            name: "ollama".into(),
        }))
        .unwrap();
    let err = scoped
        .register_provider(Arc::new(DummyProvider {
            name: "ollama".into(),
        }))
        .expect_err("dup");
    assert!(matches!(err, PluginError::DuplicateRegistration { .. }));
}
