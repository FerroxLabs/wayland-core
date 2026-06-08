//! G.6 — register the two IJFW system-prompt rules:
//!  - `IJFW-CLAUDE.md` (Claude-specific project rules)
//!  - `universal/ijfw-rules.md` (universal cross-platform rules)
//!
//! Both bodies are embedded via `include_str!` from the committed IJFW
//! snapshot at `snapshots/ijfw-source/`.

use wcore_plugin_api::{PluginContext, PluginResult, RuleScope, RuleSpec};

/// Tuple of `(name, scope, content)` for each rule registered.
const RULES: &[(&str, RuleScope, &str)] = &[
    (
        "IJFW-CLAUDE.md",
        RuleScope::ProjectScoped,
        include_str!("../snapshots/ijfw-source/claude/rules/IJFW-CLAUDE.md"),
    ),
    (
        "universal/ijfw-rules.md",
        RuleScope::Universal,
        include_str!("../snapshots/ijfw-source/universal/ijfw-rules.md"),
    ),
];

/// Number of IJFW rules the plugin registers — exposed for tests.
pub const RULE_COUNT: usize = 2;

/// Register both IJFW rules through `ctx.rules`. Manifest declares
/// `register_rules = true`, so the registry must be present.
pub fn register(ctx: &mut PluginContext<'_>) -> PluginResult<()> {
    // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error
    // (was `.expect(...)` panic). See agents.rs for the rationale.
    let registry =
        ctx.rules
            .as_mut()
            .ok_or_else(|| wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-ijfw".into(),
                surface: "rules".into(),
            })?;
    for (name, scope, content) in RULES {
        registry.register_rule(RuleSpec {
            name: (*name).to_string(),
            content: (*content).to_string(),
            scope: *scope,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_count_matches_constant() {
        assert_eq!(RULES.len(), RULE_COUNT);
    }

    #[test]
    fn rule_bodies_are_nonempty() {
        for (name, _, content) in RULES {
            assert!(!content.is_empty(), "rule {name} body must be non-empty");
        }
    }
}
