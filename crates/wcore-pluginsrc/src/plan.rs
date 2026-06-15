//! The install spine. An [`InstallPlan`] is a pure, inspectable description of
//! exactly what installing a plugin will do — what it adds, what it will be
//! allowed to spawn, what it ignores — produced BEFORE any disk mutation or
//! process spawn. `--dry-run` prints it; an interactive install renders it as
//! the consent surface. No method here touches the filesystem or spawns.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wcore_plugin_api::mcp_server_spec::McpTransport;

use crate::model::{CanonicalDraft, CompatibilityGrade, IgnoredFeature, ResolvedVersion};

/// One namespaced component the install will contribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedComponent {
    pub kind: String, // "skill" | "command" | "agent"
    pub name: String, // already namespaced: "<marketplace>/<plugin>:<component>"
}

/// What an MCP server WOULD spawn, surfaced for consent. Env VALUES are never
/// included — only the keys, so a token's name shows without leaking its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSpawnPreview {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub transport_kind: String, // "stdio" | "sse" | "http"
}

/// A namespaced component that already exists from another installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collision {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPlan {
    pub marketplace: String,
    pub plugin: String,
    pub resolved_version: ResolvedVersion,
    pub adds: Vec<AddedComponent>,
    pub spawns: Vec<McpSpawnPreview>,
    pub ignored: Vec<IgnoredFeature>,
    pub namespace_collisions: Vec<Collision>,
    pub grade: CompatibilityGrade,
    pub store_path: PathBuf,
}

impl InstallPlan {
    /// Build a plan from a lowered draft. Pure: no IO, no spawn. Borrows the
    /// draft so it survives for the commit step.
    pub fn from_draft(
        draft: &CanonicalDraft,
        marketplace: &str,
        store_path: impl Into<PathBuf>,
    ) -> Self {
        let ns = &draft.namespace;
        let mut adds = Vec::new();
        for s in &draft.skills {
            adds.push(AddedComponent {
                kind: "skill".to_string(),
                name: format!("{ns}:{}", s.name),
            });
        }
        for c in &draft.commands {
            adds.push(AddedComponent {
                kind: "command".to_string(),
                name: format!("{ns}:{}", c.name),
            });
        }
        for a in &draft.agents {
            adds.push(AddedComponent {
                kind: "agent".to_string(),
                name: format!("{ns}:{}", a.name),
            });
        }

        let spawns = draft.mcp_servers.iter().map(spawn_preview).collect();
        let grade = draft.effective_grade();

        Self {
            marketplace: marketplace.to_string(),
            plugin: draft.name.clone(),
            resolved_version: draft.version.clone(),
            adds,
            spawns,
            ignored: draft.ignored.clone(),
            namespace_collisions: Vec::new(),
            grade,
            store_path: store_path.into(),
        }
    }

    /// Human-readable consent text. This is what a user approves before any
    /// disk write or process spawn.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Install {}@{} ({}) — {:?}\n",
            self.plugin,
            self.marketplace,
            version_str(&self.resolved_version),
            self.grade,
        ));

        let (skills, commands, agents) = self.add_counts();
        out.push_str(&format!(
            "  adds: {skills} skill(s), {agents} agent(s), {commands} command(s)\n"
        ));

        if !self.spawns.is_empty() {
            out.push_str("  will be allowed to spawn:\n");
            for s in &self.spawns {
                let env = if s.env_keys.is_empty() {
                    String::new()
                } else {
                    format!("  (env: {})", s.env_keys.join(", "))
                };
                out.push_str(&format!(
                    "    - {}: {} {} [{}]{}\n",
                    s.name,
                    s.command,
                    s.args.join(" "),
                    s.transport_kind,
                    env
                ));
            }
        }

        if !self.namespace_collisions.is_empty() {
            out.push_str("  name collisions (will be namespaced):\n");
            for c in &self.namespace_collisions {
                out.push_str(&format!("    - {} {}\n", c.kind, c.name));
            }
        }

        if !self.ignored.is_empty() {
            out.push_str("  ignores (unsupported in v1):\n");
            for i in &self.ignored {
                out.push_str(&format!("    - {}: {}\n", i.kind, i.detail));
            }
        }
        out
    }

    fn add_counts(&self) -> (usize, usize, usize) {
        let skills = self.adds.iter().filter(|a| a.kind == "skill").count();
        let commands = self.adds.iter().filter(|a| a.kind == "command").count();
        let agents = self.adds.iter().filter(|a| a.kind == "agent").count();
        (skills, commands, agents)
    }
}

fn spawn_preview(s: &crate::model::McpServerDraft) -> McpSpawnPreview {
    let (command, args, kind) = match &s.transport {
        McpTransport::Stdio { command, args } => (command.clone(), args.clone(), "stdio"),
        McpTransport::Sse { url } => (url.clone(), Vec::new(), "sse"),
        McpTransport::Http { url } => (url.clone(), Vec::new(), "http"),
    };
    McpSpawnPreview {
        name: s.name.clone(),
        command,
        args,
        env_keys: s.env.keys().cloned().collect(), // BTreeMap → already sorted
        transport_kind: kind.to_string(),
    }
}

fn version_str(v: &ResolvedVersion) -> String {
    match v {
        ResolvedVersion::Explicit(s) => s.clone(),
        ResolvedVersion::CommitSha(s) => {
            let short = s.get(..12).unwrap_or(s.as_str());
            format!("sha:{short}")
        }
        ResolvedVersion::Unknown => "unknown".to_string(),
    }
}
