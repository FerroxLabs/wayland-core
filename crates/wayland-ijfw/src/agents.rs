//! G.4 — register the 3 IJFW agent profiles (architect, builder, scout).
//!
//! Each agent's body is a Markdown file with a YAML frontmatter block.
//! We embed the file contents at compile time via `include_str!` (from
//! the committed `snapshots/ijfw-source/claude/agents/` directory) and
//! parse the frontmatter into an [`AgentManifest`] at registration
//! time.

use serde::Deserialize;
use wcore_plugin_api::{AgentManifest, PluginContext, PluginError, PluginResult};

/// Snapshot Markdown source for each IJFW agent. The tuple is
/// `(name, body)` where `body` is the full SKILL/agent .md including
/// the YAML frontmatter block.
const AGENT_FILES: &[(&str, &str)] = &[
    (
        "architect",
        include_str!("../snapshots/ijfw-source/claude/agents/architect.md"),
    ),
    (
        "builder",
        include_str!("../snapshots/ijfw-source/claude/agents/builder.md"),
    ),
    (
        "scout",
        include_str!("../snapshots/ijfw-source/claude/agents/scout.md"),
    ),
];

/// Minimal frontmatter shape we care about for `AgentManifest` mapping.
/// Mirrors the IJFW agent .md frontmatter convention
/// (`name`, `description`, `model`, `allowed-tools`).
#[derive(Debug, Deserialize)]
struct AgentFrontmatter {
    #[allow(dead_code)]
    name: String,
    description: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
}

/// Number of IJFW agents the plugin registers — exposed for tests.
pub const AGENT_COUNT: usize = 3;

/// Build the `AgentManifest` for one of the embedded agent files.
fn build_manifest(name: &str, body: &str) -> Result<AgentManifest, PluginError> {
    let (front, prose) = split_frontmatter(body).ok_or_else(|| PluginError::ManifestSchema {
        reason: format!("wayland-ijfw: agent {name} is missing YAML frontmatter block"),
    })?;
    let fm: AgentFrontmatter =
        serde_yaml::from_str(front).map_err(|e| PluginError::ManifestSchema {
            reason: format!("wayland-ijfw: agent {name} frontmatter parse error: {e}"),
        })?;
    let allowed_tools = fm
        .allowed_tools
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(AgentManifest {
        name: name.to_string(),
        description: fm.description,
        model: fm.model,
        system_prompt: prose.trim_start_matches('\n').to_string(),
        allowed_tools,
        max_turns: None,
    })
}

/// Split a Markdown body into `(frontmatter, body)` if it starts with a
/// `---\n…\n---\n` block. Returns `None` when no frontmatter is present.
///
/// Leading HTML comments (`<!-- ... -->`) are skipped before the
/// frontmatter fence is matched — mirrors the skill loader's tolerance
/// for IJFW's narration-suppression markers.
fn split_frontmatter(body: &str) -> Option<(&str, &str)> {
    let mut cursor = body;
    while let Some(rest) = cursor.strip_prefix("<!--") {
        let close = rest.find("-->")?;
        cursor = rest[close + 3..].trim_start_matches(['\r', '\n']);
    }
    let rest = cursor.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let (front, tail) = rest.split_at(end);
    let body_rest = tail.strip_prefix("\n---").unwrap_or(tail);
    Some((front, body_rest))
}

/// Register all IJFW agents through `ctx.agents`. Manifest declares
/// `register_agents = true`, so the registry must be present.
pub fn register(ctx: &mut PluginContext<'_>) -> PluginResult<()> {
    // Wave RB STABILITY MINOR #13: typed error instead of panic when
    // the host fails to populate `ctx.agents`. The plugin's manifest
    // declares `register_agents = true`, so a missing registry means
    // the host is misconfigured — surface it through the normal
    // PluginResult channel instead of crashing initialize_all.
    let registry =
        ctx.agents
            .as_mut()
            .ok_or_else(|| wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-ijfw".into(),
                surface: "agents".into(),
            })?;
    for (name, body) in AGENT_FILES {
        let manifest = build_manifest(name, body)?;
        registry.register_agent(manifest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_split_handles_standard_format() {
        let body = "---\nname: test\ndescription: x\n---\n\nbody here";
        let (front, rest) = split_frontmatter(body).unwrap();
        assert!(front.contains("name: test"));
        assert!(rest.contains("body here"));
    }

    #[test]
    fn each_vendored_agent_parses() {
        for (name, body) in AGENT_FILES {
            build_manifest(name, body)
                .unwrap_or_else(|e| panic!("agent {name} failed to parse: {e}"));
        }
    }

    #[test]
    fn agent_count_matches_vendored_files() {
        assert_eq!(AGENT_FILES.len(), AGENT_COUNT);
    }
}
