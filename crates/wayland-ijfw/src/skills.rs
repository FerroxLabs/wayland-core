//! G.5 — register 22 `ijfw-*` skills.
//!
//! Each skill is embedded via `include_str!` from the committed IJFW
//! snapshot at `snapshots/ijfw-source/claude/skills/<name>/SKILL.md`.
//! We parse the YAML
//! frontmatter at registration time to populate the `BundledSkillSpec`
//! fields (description, when_to_use, etc.); the prose body becomes
//! `BundledSkillSpec.content`.

use serde::Deserialize;
use wcore_plugin_api::{BundledSkillSpec, PluginContext, PluginError, PluginResult};

/// 22 IJFW skills enumerated at compile time. Each entry is
/// `(name, SKILL.md contents)`.
///
/// Listing the names explicitly (rather than walking the directory in
/// build.rs) is the pragmatic choice for the anchor plugin — IJFW's
/// 22-skill surface is stable, the names are listed in the plan, and
/// adding new skills is a deliberate vendoring decision.
const SKILL_FILES: &[(&str, &str)] = &[
    (
        "ijfw-agents-md",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-agents-md/SKILL.md"),
    ),
    (
        "ijfw-auto-memorize",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-auto-memorize/SKILL.md"),
    ),
    (
        "ijfw-commit",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-commit/SKILL.md"),
    ),
    (
        "ijfw-compress",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-compress/SKILL.md"),
    ),
    (
        "ijfw-compute",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-compute/SKILL.md"),
    ),
    (
        "ijfw-core",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-core/SKILL.md"),
    ),
    (
        "ijfw-critique",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-critique/SKILL.md"),
    ),
    (
        "ijfw-cross-audit",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-cross-audit/SKILL.md"),
    ),
    (
        "ijfw-dashboard",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-dashboard/SKILL.md"),
    ),
    (
        "ijfw-debug",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-debug/SKILL.md"),
    ),
    (
        "ijfw-design",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-design/SKILL.md"),
    ),
    (
        "ijfw-handoff",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-handoff/SKILL.md"),
    ),
    (
        "ijfw-memory-audit",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-memory-audit/SKILL.md"),
    ),
    (
        "ijfw-metrics",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-metrics/SKILL.md"),
    ),
    (
        "ijfw-plan-check",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-plan-check/SKILL.md"),
    ),
    (
        "ijfw-preflight",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-preflight/SKILL.md"),
    ),
    (
        "ijfw-recall",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-recall/SKILL.md"),
    ),
    (
        "ijfw-review",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-review/SKILL.md"),
    ),
    (
        "ijfw-summarize",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-summarize/SKILL.md"),
    ),
    (
        "ijfw-team",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-team/SKILL.md"),
    ),
    (
        "ijfw-update",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-update/SKILL.md"),
    ),
    (
        "ijfw-workflow",
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-workflow/SKILL.md"),
    ),
];

/// Number of IJFW skills the plugin registers — exposed for tests.
pub const SKILL_COUNT: usize = 22;

/// Frontmatter shape we extract from `SKILL.md` files.
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    #[serde(default)]
    name: Option<String>,
    description: String,
    #[serde(default)]
    when_to_use: Option<String>,
    #[serde(default)]
    argument_hint: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    agent: Option<String>,
}

/// Split `---\nFRONT\n---\nBODY` into `(front, body)`.
///
/// Leading HTML comments (`<!-- ... -->`) — a convention IJFW uses for
/// narration-suppression markers — are skipped before the frontmatter
/// fence is matched.
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

fn build_spec(name: &str, body: &str) -> Result<BundledSkillSpec, PluginError> {
    let (front, prose) = split_frontmatter(body).ok_or_else(|| PluginError::ManifestSchema {
        reason: format!("wayland-ijfw: skill {name} is missing YAML frontmatter block"),
    })?;
    let fm: SkillFrontmatter =
        serde_yaml::from_str(front).map_err(|e| PluginError::ManifestSchema {
            reason: format!("wayland-ijfw: skill {name} frontmatter parse error: {e}"),
        })?;
    // `name` from frontmatter is honored when present, but the canonical
    // registration name comes from the directory listing — that is what
    // operators reference at the CLI. They should match in healthy IJFW
    // distributions; we don't fail if they don't.
    let canonical_name = fm.name.unwrap_or_else(|| name.to_string());
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
    Ok(BundledSkillSpec {
        name: canonical_name,
        description: fm.description,
        when_to_use: fm.when_to_use,
        argument_hint: fm.argument_hint,
        allowed_tools,
        model: fm.model,
        disable_model_invocation: false,
        user_invocable: true,
        context: None,
        agent: fm.agent,
        files: Vec::new(),
        content: prose.trim_start_matches('\n').to_string(),
    })
}

/// Register all 22 IJFW skills through `ctx.skills`. Manifest declares
/// `register_skills = true`, so the registry must be present.
pub fn register(ctx: &mut PluginContext<'_>) -> PluginResult<()> {
    // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error.
    let registry =
        ctx.skills
            .as_mut()
            .ok_or_else(|| wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-ijfw".into(),
                surface: "skills".into(),
            })?;
    for (name, body) in SKILL_FILES {
        let spec = build_spec(name, body)?;
        registry.register_skill(spec)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_skill_count_matches_constant() {
        assert_eq!(SKILL_FILES.len(), SKILL_COUNT);
    }

    #[test]
    fn each_vendored_skill_parses() {
        for (name, body) in SKILL_FILES {
            build_spec(name, body).unwrap_or_else(|e| panic!("skill {name} failed to parse: {e}"));
        }
    }

    #[test]
    fn skill_names_are_unique() {
        let mut names: Vec<&str> = SKILL_FILES.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let dedup_len = names.iter().fold(0usize, |acc, _| acc + 1);
        names.dedup();
        assert_eq!(
            names.len(),
            dedup_len,
            "duplicate skill name in SKILL_FILES"
        );
    }
}
