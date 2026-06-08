//! G.8 — Full-surface acceptance test.
//!
//! Loads `WaylandIjfw` via its public `Plugin` trait, builds a
//! `PluginContext` wired to recording host adapters that live in this
//! test file, runs `initialize()`, and asserts every `register_*`
//! surface the manifest permits records at least one entry.
//!
//! Providers are deliberately *not* registered (that's wayland-ollama's
//! job) — the assertion that the providers vector is empty enforces
//! the boundary.

use std::collections::HashMap;
use std::sync::Arc;

use wayland_ijfw::{WaylandIjfw, agents, hooks, mcp, rules, skills, tools};
use wcore_plugin_api::registry::agents::{AgentRegistrar, ScopedAgentRegistry};
use wcore_plugin_api::registry::config::{ConfigReader, ScopedConfigReader};
use wcore_plugin_api::registry::hooks::{HookPhase, HookRegistrar, ScopedHookRegistry};
use wcore_plugin_api::registry::logger::ScopedPluginLogger;
use wcore_plugin_api::registry::mcp::{McpRegistrar, ScopedMcpRegistry};
use wcore_plugin_api::registry::memory::{MemoryHost, ScopedMemoryClient};
use wcore_plugin_api::registry::providers::{
    PluginProvider, ProviderRegistrar, ScopedProviderRegistry,
};
use wcore_plugin_api::registry::rules::{RuleRegistrar, ScopedRuleRegistry};
use wcore_plugin_api::registry::skills::{ScopedSkillRegistry, SkillRegistrar};
use wcore_plugin_api::registry::tools::{ScopedToolRegistry, ToolRegistrar};
use wcore_plugin_api::{
    AgentManifest, BundledSkillSpec, McpServerSpec, MemoryItem, MemoryQuery, Partition, Plugin,
    PluginContext, RuleSpec,
};

#[derive(Default)]
struct RecordingTools {
    seen: Vec<String>,
}
impl ToolRegistrar for RecordingTools {
    fn host_register(
        &mut self,
        fq: String,
        _tool: wcore_plugin_api::tool::PluginTool,
    ) -> Result<(), String> {
        self.seen.push(fq);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingHooks {
    seen: Vec<(HookPhase, String)>,
}
impl HookRegistrar for RecordingHooks {
    fn host_register_hook(&mut self, phase: HookPhase, name: String) -> Result<(), String> {
        self.seen.push((phase, name));
        Ok(())
    }
}

#[derive(Default)]
struct RecordingAgents {
    seen: Vec<AgentManifest>,
}
impl AgentRegistrar for RecordingAgents {
    fn host_register_agent(&mut self, agent: AgentManifest) -> Result<(), String> {
        self.seen.push(agent);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingSkills {
    seen: Vec<BundledSkillSpec>,
}
impl SkillRegistrar for RecordingSkills {
    fn host_register_skill(&mut self, skill: BundledSkillSpec) -> Result<(), String> {
        self.seen.push(skill);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingRules {
    seen: Vec<RuleSpec>,
}
impl RuleRegistrar for RecordingRules {
    fn host_register_rule(&mut self, rule: RuleSpec) -> Result<(), String> {
        self.seen.push(rule);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingMcp {
    seen: Vec<McpServerSpec>,
}
impl McpRegistrar for RecordingMcp {
    fn host_register_mcp_server(&mut self, server: McpServerSpec) -> Result<(), String> {
        self.seen.push(server);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingProviders {
    seen: Vec<String>,
}
impl ProviderRegistrar for RecordingProviders {
    fn host_register_provider(&mut self, provider: Arc<dyn PluginProvider>) -> Result<(), String> {
        self.seen.push(provider.provider_name().to_string());
        Ok(())
    }
}

struct NullConfig;
impl ConfigReader for NullConfig {
    fn get_raw(&self, _key: &str) -> Option<serde_json::Value> {
        None
    }
}

#[derive(Default)]
struct NullMemory {
    writes: HashMap<Partition, Vec<MemoryItem>>,
}
impl MemoryHost for NullMemory {
    fn host_read(
        &self,
        _partition: Partition,
        _query: &MemoryQuery,
    ) -> Result<Vec<MemoryItem>, String> {
        Ok(Vec::new())
    }
    fn host_write(&mut self, partition: Partition, item: MemoryItem) -> Result<(), String> {
        self.writes.entry(partition).or_default().push(item);
        Ok(())
    }
}

#[tokio::test]
async fn wayland_ijfw_exercises_every_register_method() {
    let plugin = WaylandIjfw;
    let manifest = plugin.manifest().clone();

    let mut tools_host = RecordingTools::default();
    let mut hooks_host = RecordingHooks::default();
    let mut agents_host = RecordingAgents::default();
    let mut skills_host = RecordingSkills::default();
    let mut rules_host = RecordingRules::default();
    let mut mcp_host = RecordingMcp::default();
    let mut providers_host = RecordingProviders::default();
    let config_host = NullConfig;
    let mut memory_host = NullMemory::default();

    let tools = ScopedToolRegistry::new(&manifest, &mut tools_host).ok();
    let hooks = ScopedHookRegistry::new(&manifest, &mut hooks_host).ok();
    let agents = ScopedAgentRegistry::new(&manifest, &mut agents_host).ok();
    let skills = ScopedSkillRegistry::new(&manifest, &mut skills_host).ok();
    let rules = ScopedRuleRegistry::new(&manifest, &mut rules_host).ok();
    let mcp_servers = ScopedMcpRegistry::new(&manifest, &mut mcp_host).ok();
    // Providers are deliberately not granted: the wayland-ijfw manifest
    // declares `register_providers = false`, so this Option should be
    // `None` after access-gate runs.
    let providers = ScopedProviderRegistry::new(&manifest, &mut providers_host).ok();

    let config = ScopedConfigReader::new(&config_host);
    let logger = ScopedPluginLogger::new(&manifest.plugin.name);
    let memory = ScopedMemoryClient::new(&manifest, &mut memory_host);

    let mut ctx = PluginContext {
        manifest: &manifest,
        tools,
        hooks,
        agents,
        skills,
        rules,
        mcp_servers,
        providers,
        browser: None,
        cua: None,
        user_models: None,
        sandbox: None,
        config,
        logger,
        memory,
    };

    plugin
        .initialize(&mut ctx)
        .await
        .expect("initialize must succeed");

    // tools — at least the two names declared in tools::TOOL_NAMES,
    // each fully-qualified with the `ijfw::` namespace.
    assert!(
        tools_host.seen.contains(&"ijfw::ijfw_run".to_string()),
        "ijfw::ijfw_run not registered: {:?}",
        tools_host.seen
    );
    assert!(
        tools_host
            .seen
            .contains(&"ijfw::ijfw_update_apply".to_string())
    );
    assert_eq!(tools_host.seen.len(), tools::TOOL_NAMES.len());

    // hooks — exactly the 5 declared phases.
    assert_eq!(hooks_host.seen.len(), hooks::HOOKS.len());

    // agents — exactly 3 (architect/builder/scout).
    assert_eq!(agents_host.seen.len(), agents::AGENT_COUNT);
    let names: Vec<&str> = agents_host.seen.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"architect"));
    assert!(names.contains(&"builder"));
    assert!(names.contains(&"scout"));

    // skills — the 22 ijfw-* skills.
    assert_eq!(skills_host.seen.len(), skills::SKILL_COUNT);

    // rules — IJFW-CLAUDE.md + universal/ijfw-rules.md.
    assert_eq!(rules_host.seen.len(), rules::RULE_COUNT);

    // MCP — the single IJFW server, IF the host can run it. The MCP
    // registration in `wayland_ijfw::mcp::register` deliberately skips
    // (returns Ok without registering) when `npx --version` is not on
    // PATH — Node-less hosts (e.g. the ci-linux Dockerfile) shouldn't
    // advertise an MCP server they cannot start. So accept 0 or 1.
    assert!(
        mcp_host.seen.len() <= 1,
        "expected 0 or 1 MCP registrations, got {}",
        mcp_host.seen.len()
    );
    if let Some(first) = mcp_host.seen.first() {
        assert_eq!(first.name, mcp::SERVER_NAME);
    }

    // Providers — wayland-ijfw deliberately leaves this empty (the
    // wayland-ollama plugin is the reference provider implementation).
    assert!(
        providers_host.seen.is_empty(),
        "wayland-ijfw must not register providers (boundary with wayland-ollama)"
    );
}
