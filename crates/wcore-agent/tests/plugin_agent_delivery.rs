//! v0.6.4 Task 1.2 — agent delivery via `apply_initialize_outcome`.
//!
//! Proves: given an `InitializeOutcome` carrying a plugin `AgentManifest`,
//! `apply_initialize_outcome` returns an `AppliedPluginCapabilities` whose
//! `agent_registry` resolves that agent by name.
//!
//! Also proves: duplicate manifests are silently skipped (first-wins, no
//! panic) — the "one bad plugin cannot crash boot" invariant for agents.

use wcore_agent::plugins::{InitializeOutcome, apply_initialize_outcome};
use wcore_plugin_api::AgentManifest;
use wcore_tools::registry::ToolRegistry;

/// v0.6.4 Task 1.7 — `apply_initialize_outcome` now takes a `&mut ToolRegistry`
/// (the tool-delivery sink). These agent-path tests pass a throwaway registry.
fn apply(outcome: InitializeOutcome) -> wcore_agent::plugins::AppliedPluginCapabilities {
    let mut registry = ToolRegistry::new();
    apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    )
}

fn manifest(name: &str) -> AgentManifest {
    AgentManifest {
        name: name.to_string(),
        description: format!("{name} agent"),
        model: None,
        system_prompt: format!("you are {name}"),
        allowed_tools: vec![],
        max_turns: None,
    }
}

/// `apply_initialize_outcome` resolves an agent manifest registered by a plugin.
#[test]
fn agent_from_outcome_is_resolvable_by_name() {
    let mut outcome = InitializeOutcome::default();
    outcome.agents.push(manifest("reviewer"));

    let applied = apply(outcome);

    let got = applied
        .agent_registry
        .get("reviewer")
        .expect("agent registered by plugin must be resolvable by name");
    assert_eq!(got.name, "reviewer");
    assert_eq!(got.description, "reviewer agent");
    assert_eq!(got.system_prompt, "you are reviewer");
}

/// Multiple distinct agents all land in the registry.
#[test]
fn multiple_agents_all_registered() {
    let mut outcome = InitializeOutcome::default();
    outcome.agents.push(manifest("alpha"));
    outcome.agents.push(manifest("beta"));
    outcome.agents.push(manifest("gamma"));

    let applied = apply(outcome);

    assert!(
        applied.agent_registry.get("alpha").is_some(),
        "alpha must be registered"
    );
    assert!(
        applied.agent_registry.get("beta").is_some(),
        "beta must be registered"
    );
    assert!(
        applied.agent_registry.get("gamma").is_some(),
        "gamma must be registered"
    );
    assert_eq!(applied.agent_registry.list().len(), 3);
}

/// Duplicate agent names: first wins, second is silently skipped (no panic).
#[test]
fn duplicate_agent_name_is_skipped_not_panicked() {
    let mut outcome = InitializeOutcome::default();
    outcome.agents.push(manifest("analyst"));
    // Second manifest with the same name — must not overwrite or panic.
    let mut dupe = manifest("analyst");
    dupe.description = "impostor".to_string();
    outcome.agents.push(dupe);

    let applied = apply(outcome);

    let got = applied
        .agent_registry
        .get("analyst")
        .expect("analyst must still be registered after duplicate skip");
    // First registration wins.
    assert_eq!(
        got.description, "analyst agent",
        "first registration must win"
    );
    // The duplicate was skipped — only one entry in the registry.
    assert_eq!(
        applied.agent_registry.list().len(),
        1,
        "duplicate must be skipped, not added"
    );
}

/// An empty `agents` vec produces an empty but valid registry (no crash).
#[test]
fn empty_agents_yields_empty_registry() {
    let outcome = InitializeOutcome::default();
    let applied = apply(outcome);
    assert!(
        applied.agent_registry.list().is_empty(),
        "empty outcome must yield an empty registry"
    );
}

/// Pass-through fields are preserved: skills, hooks, rules arrive unchanged.
#[test]
fn pass_through_fields_are_preserved() {
    use wcore_agent::plugins::runner::PluginHook;
    use wcore_plugin_api::registry::hooks::HookPhase;
    use wcore_plugin_api::{BundledSkillSpec, RuleScope, RuleSpec};

    let mut outcome = InitializeOutcome::default();
    outcome.hooks.push(PluginHook {
        plugin: "test-plugin".to_string(),
        phase: HookPhase::PreToolUse,
        name: "my_hook".to_string(),
    });

    let skill = BundledSkillSpec {
        name: "test-skill".to_string(),
        description: "a test skill".to_string(),
        when_to_use: None,
        argument_hint: None,
        allowed_tools: vec![],
        model: None,
        disable_model_invocation: false,
        user_invocable: true,
        context: None,
        agent: None,
        files: vec![],
        content: "do the thing".to_string(),
    };
    outcome.skills.push(skill);

    let rule = RuleSpec {
        name: "test-rule".to_string(),
        content: "always be polite".to_string(),
        scope: RuleScope::Universal,
    };
    outcome.rules.push(rule);

    let applied = apply(outcome);

    assert_eq!(
        applied.plugin_hooks.len(),
        1,
        "hooks must pass through unchanged"
    );
    assert_eq!(applied.plugin_hooks[0].name, "my_hook");
    assert!(matches!(
        applied.plugin_hooks[0].phase,
        HookPhase::PreToolUse
    ));
    assert_eq!(
        applied.plugin_skills.len(),
        1,
        "skill must survive into applied"
    );
    assert_eq!(
        applied.plugin_skills[0].name, "test-skill",
        "skill name must match"
    );

    assert_eq!(
        applied.plugin_rules.len(),
        1,
        "rule must survive into applied"
    );
    assert_eq!(
        applied.plugin_rules[0].name, "test-rule",
        "rule name must match"
    );
}
