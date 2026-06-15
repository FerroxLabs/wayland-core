//! The Wayland-native canonical plugin model that all foreign adapters lower to.
//! These types are format-blind: nothing here knows about Claude Code, Cursor,
//! or any specific vendor. Adapters produce a [`CanonicalDraft`]; the install
//! planner (Lane B) turns it into an `InstallPlan`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wcore_plugin_api::mcp_server_spec::McpTransport;

/// How compatible a plugin is once lowered. `Ord` deliberately puts the
/// WEAKEST grade lowest, so `.min()` / `sort()[0]` yields the surface a plugin
/// is least compatible on — letting a draft report its weakest dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CompatibilityGrade {
    /// Declared something we cannot faithfully run (weakest).
    UnsupportedBehavior,
    /// Installs, but hooks are dropped (v1 does not run foreign hooks).
    HooksIgnored,
    /// Pure MCP-server contribution — tools light up, no other surfaces.
    McpCompatible,
    /// Skills / agents / commands map cleanly (strongest).
    ContentCompatible,
}

/// How a plugin's version was resolved (mirrors Claude Code's resolution order:
/// explicit manifest/entry version → git commit SHA → unknown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedVersion {
    Explicit(String),
    CommitSha(String),
    Unknown,
}

/// One foreign feature that did not survive lowering, recorded for the
/// lossy-translation report so degradation is never silent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredFeature {
    pub kind: String,
    pub detail: String,
}

/// A non-blocking risk surfaced on the [`InstallPlan`](crate::InstallPlan): it
/// is graded and shown, never auto-blocked — the user decides. Lane E2 emits
/// `prompt-risk` warnings (injection / credential markers found in asset text);
/// Lane E3 emits `unsigned-source` trust warnings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanWarning {
    /// Category, e.g. `"prompt-risk"` or `"unsigned-source"`.
    pub kind: String,
    /// The asset the warning is about (`"skill:<name>"`, `"agent:<name>"`, …),
    /// or empty for a plan-level warning.
    pub component: String,
    pub detail: String,
}

/// A skill copied verbatim (`<rel_dir>/SKILL.md` + supporting files).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillAsset {
    pub name: String,
    pub rel_dir: PathBuf,
}

/// A flat-markdown command (Claude Code `commands/*.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandAsset {
    pub name: String,
    pub rel_file: PathBuf,
}

/// An agent lowered to the fields Wayland's `AgentManifest` can hold. Foreign
/// agent fields with no Wayland equivalent become [`IgnoredFeature`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAsset {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    pub max_turns: Option<u32>,
}

/// An MCP server entry with `${VARS}` still unresolved (resolution happens at
/// runtime load against the install dir — see Lane D's `var_subst`).
///
/// Not `PartialEq`/`Eq`: the upstream `McpTransport` derives neither.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDraft {
    pub name: String,
    pub transport: McpTransport,
    pub env: BTreeMap<String, String>,
}

/// One plugin source entry from a marketplace `plugins[]` element, normalized
/// across the relative-path / github / url / git-subdir / npm shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEntry {
    pub name: String,
    pub kind: SourceKind,
    pub strict: bool,
    pub declared_version: Option<String>,
    /// Human-readable blurb from the marketplace catalog, if it declares one.
    /// Used by the browse UI; not needed for install resolution.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    RelativePath(PathBuf),
    Github {
        repo: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        sha: Option<String>,
    },
    Url {
        url: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        sha: Option<String>,
    },
    GitSubdir {
        url: String,
        path: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        sha: Option<String>,
    },
    /// npm source — parsed but deferred to v1.1 (needs a Node toolchain).
    Npm {
        package: String,
        version: Option<String>,
        registry: Option<String>,
    },
}

/// A foreign plugin lowered to Wayland-native form, not yet written to disk.
///
/// Not `PartialEq`/`Eq`: holds `McpServerDraft`, whose `McpTransport` is neither.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalDraft {
    pub name: String,
    /// `<marketplace>/<plugin>` — the namespacing prefix for all components.
    pub namespace: String,
    pub version: ResolvedVersion,
    pub skills: Vec<SkillAsset>,
    pub commands: Vec<CommandAsset>,
    pub agents: Vec<AgentAsset>,
    pub mcp_servers: Vec<McpServerDraft>,
    pub ignored: Vec<IgnoredFeature>,
    /// Non-blocking risks surfaced for consent (Lane E2/E3).
    pub warnings: Vec<PlanWarning>,
    pub grade: CompatibilityGrade,
}

impl CanonicalDraft {
    pub fn empty(marketplace: &str, plugin: &str) -> Self {
        Self {
            name: plugin.to_string(),
            namespace: format!("{marketplace}/{plugin}"),
            version: ResolvedVersion::Unknown,
            skills: Vec::new(),
            commands: Vec::new(),
            agents: Vec::new(),
            mcp_servers: Vec::new(),
            ignored: Vec::new(),
            warnings: Vec::new(),
            grade: CompatibilityGrade::ContentCompatible,
        }
    }

    /// The weakest surface present after lowering. A draft that drops hooks
    /// can never grade above `HooksIgnored`; a draft that is MCP-only grades
    /// `McpCompatible`.
    pub fn effective_grade(&self) -> CompatibilityGrade {
        let mut g = self.grade;
        if self.ignored.iter().any(|i| i.kind == "hooks") {
            g = g.min(CompatibilityGrade::HooksIgnored);
        }
        let content_empty =
            self.skills.is_empty() && self.agents.is_empty() && self.commands.is_empty();
        if !self.mcp_servers.is_empty() && content_empty {
            g = g.min(CompatibilityGrade::McpCompatible);
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grade_orders_worst_first() {
        let mut g = [
            CompatibilityGrade::ContentCompatible,
            CompatibilityGrade::UnsupportedBehavior,
            CompatibilityGrade::HooksIgnored,
        ];
        g.sort();
        assert_eq!(
            g[0],
            CompatibilityGrade::UnsupportedBehavior,
            "worst grade must sort first so a draft reports its weakest surface"
        );
    }

    #[test]
    fn draft_namespaced_name_combines_marketplace_and_plugin() {
        let d = CanonicalDraft::empty("acme", "formatter");
        assert_eq!(d.namespace, "acme/formatter");
    }

    #[test]
    fn effective_grade_drops_to_hooks_ignored_when_hooks_present() {
        let mut d = CanonicalDraft::empty("acme", "p");
        d.skills.push(SkillAsset {
            name: "s".into(),
            rel_dir: "skills/s".into(),
        });
        d.ignored.push(IgnoredFeature {
            kind: "hooks".into(),
            detail: "PostToolUse x1".into(),
        });
        assert_eq!(d.effective_grade(), CompatibilityGrade::HooksIgnored);
    }

    #[test]
    fn effective_grade_mcp_only_is_mcp_compatible() {
        let mut d = CanonicalDraft::empty("reg", "fetch");
        d.mcp_servers.push(McpServerDraft {
            name: "fetch".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-server-fetch".into()],
            },
            env: BTreeMap::new(),
        });
        assert_eq!(d.effective_grade(), CompatibilityGrade::McpCompatible);
    }
}
