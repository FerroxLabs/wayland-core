//! Embedded TOML manifests for the 13 built-in agents.
//!
//! Each manifest is loaded from `templates/agents/<name>.toml` at
//! compile-time via `include_str!`. Parsing happens once on first access
//! and is cached via `OnceLock`.

use std::sync::OnceLock;
use wcore_plugin_api::AgentManifest;

pub const NAMES: &[&str] = &[
    // 3.A.2 code tier
    "architect",
    "debugger",
    "security-auditor",
    "refactor-buddy",
    "qa-engineer",
    // 3.A.3 research + writing tier
    "deep-researcher",
    "fact-checker",
    "copywriter",
    "technical-writer",
    "humanizer",
    // 3.A.4 ops + creative tier
    "incident-commander",
    "deploy-pilot",
    "brand-strategist",
];

const TOML_SOURCES: &[(&str, &str)] = &[
    (
        "architect",
        include_str!("../templates/agents/architect.toml"),
    ),
    (
        "debugger",
        include_str!("../templates/agents/debugger.toml"),
    ),
    (
        "security-auditor",
        include_str!("../templates/agents/security-auditor.toml"),
    ),
    (
        "refactor-buddy",
        include_str!("../templates/agents/refactor-buddy.toml"),
    ),
    (
        "qa-engineer",
        include_str!("../templates/agents/qa-engineer.toml"),
    ),
    (
        "deep-researcher",
        include_str!("../templates/agents/deep-researcher.toml"),
    ),
    (
        "fact-checker",
        include_str!("../templates/agents/fact-checker.toml"),
    ),
    (
        "copywriter",
        include_str!("../templates/agents/copywriter.toml"),
    ),
    (
        "technical-writer",
        include_str!("../templates/agents/technical-writer.toml"),
    ),
    (
        "humanizer",
        include_str!("../templates/agents/humanizer.toml"),
    ),
    (
        "incident-commander",
        include_str!("../templates/agents/incident-commander.toml"),
    ),
    (
        "deploy-pilot",
        include_str!("../templates/agents/deploy-pilot.toml"),
    ),
    (
        "brand-strategist",
        include_str!("../templates/agents/brand-strategist.toml"),
    ),
];

static CACHE: OnceLock<Vec<AgentManifest>> = OnceLock::new();

pub fn all() -> &'static [AgentManifest] {
    CACHE.get_or_init(|| {
        TOML_SOURCES
            .iter()
            .map(|(name, src)| {
                toml::from_str::<AgentManifest>(src)
                    .unwrap_or_else(|e| panic!("invalid built-in agent manifest {name}: {e}"))
            })
            .collect()
    })
}
