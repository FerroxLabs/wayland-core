//! Claude Code plugin-format adapter. Lowers a `.claude-plugin/plugin.json`
//! plugin (skills / commands / agents / `.mcp.json`) into a [`CanonicalDraft`].
//!
//! Foreign-format knowledge is confined here. Hooks are detected and recorded
//! as an [`IgnoredFeature`] (v1 does not run foreign hooks) so the grade drops
//! honestly to `HooksIgnored` rather than silently pretending parity.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use wcore_plugin_api::mcp_server_spec::McpTransport;

use crate::Result;
use crate::adapter::{PluginFormatAdapter, detect_format};
use crate::error::PluginSrcError;
use crate::frontmatter::{frontmatter_name, split_frontmatter};
use crate::model::{
    AgentAsset, CanonicalDraft, CommandAsset, IgnoredFeature, McpServerDraft, ResolvedVersion,
    SkillAsset, SourceEntry,
};

pub struct ClaudeCodeAdapter;

impl PluginFormatAdapter for ClaudeCodeAdapter {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn detect(&self, root: &Path) -> bool {
        detect_format(root).as_deref() == Some("claude-code")
    }

    fn lower(&self, marketplace: &str, entry: &SourceEntry, root: &Path) -> Result<CanonicalDraft> {
        let manifest = read_plugin_json(root)?;
        let name = manifest.name.clone().unwrap_or_else(|| entry.name.clone());
        let mut draft = CanonicalDraft::empty(marketplace, &name);

        draft.version = match manifest
            .version
            .clone()
            .or_else(|| entry.declared_version.clone())
        {
            Some(v) => ResolvedVersion::Explicit(v),
            None => ResolvedVersion::Unknown,
        };

        lower_skills(root, &mut draft)?;
        lower_commands(root, &mut draft)?;
        lower_agents(root, &mut draft)?;
        lower_mcp_servers(root, &manifest, &mut draft)?;

        // Lane E2: scan the prompt-asset text (skill/command bodies + agent
        // prompts) for injection / credential markers. Non-blocking — the
        // warnings ride on the InstallPlan consent surface.
        draft.warnings = crate::scan::scan_draft_assets(root, &draft);

        // Hooks are not run in v1: record them so the grade is honest.
        if root.join("hooks/hooks.json").is_file()
            || manifest.hooks.as_ref().is_some_and(|v| !v.is_null())
        {
            draft.ignored.push(IgnoredFeature {
                kind: "hooks".to_string(),
                detail: "plugin declares hooks (not run in v1)".to_string(),
            });
        }

        draft.grade = draft.effective_grade();
        Ok(draft)
    }
}

/// Permissive view of `.claude-plugin/plugin.json`. Unknown fields are ignored,
/// matching Claude Code's load-tolerant behavior.
#[derive(Debug, Default, Deserialize)]
struct ClaudePluginJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    hooks: Option<serde_json::Value>,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Option<serde_json::Value>,
}

fn read_plugin_json(root: &Path) -> Result<ClaudePluginJson> {
    let p = root.join(".claude-plugin/plugin.json");
    if !p.is_file() {
        // A manifest is optional in Claude Code; default to auto-discovery.
        return Ok(ClaudePluginJson::default());
    }
    let txt = fs::read_to_string(&p)?;
    serde_json::from_str(&txt)
        .map_err(|e| PluginSrcError::PluginManifest(format!("{}: {e}", p.display())))
}

fn lower_skills(root: &Path, draft: &mut CanonicalDraft) -> Result<()> {
    let dir = root.join("skills");
    if !dir.is_dir() {
        return Ok(());
    }
    for ent in fs::read_dir(&dir)?.flatten() {
        let p = ent.path();
        let skill_md = p.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let basename = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = frontmatter_name(&skill_md).unwrap_or_else(|| basename.clone());
        draft.skills.push(SkillAsset {
            name,
            rel_dir: PathBuf::from("skills").join(&basename),
        });
    }
    Ok(())
}

fn lower_commands(root: &Path, draft: &mut CanonicalDraft) -> Result<()> {
    let dir = root.join("commands");
    if !dir.is_dir() {
        return Ok(());
    }
    for ent in fs::read_dir(&dir)?.flatten() {
        let p = ent.path();
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        draft.commands.push(CommandAsset {
            name: stem,
            rel_file: PathBuf::from("commands").join(file),
        });
    }
    Ok(())
}

fn lower_agents(root: &Path, draft: &mut CanonicalDraft) -> Result<()> {
    let dir = root.join("agents");
    if !dir.is_dir() {
        return Ok(());
    }
    for ent in fs::read_dir(&dir)?.flatten() {
        let p = ent.path();
        if p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let content = fs::read_to_string(&p)?;
        let (fm, body) = split_frontmatter(&content);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        draft.agents.push(lower_agent(
            fm.as_deref(),
            &body,
            &stem,
            &mut draft.ignored,
        )?);
    }
    Ok(())
}

/// Claude Code agent frontmatter. `tools` / `disallowedTools` may be a
/// comma-separated string or a YAML list. Fields with no `AgentManifest`
/// equivalent are captured here only to report them as ignored.
#[derive(Debug, Default, Deserialize)]
struct ClaudeAgentFm {
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    #[serde(rename = "maxTurns")]
    max_turns: Option<u32>,
    tools: Option<serde_yaml::Value>,
    #[serde(rename = "disallowedTools")]
    disallowed_tools: Option<serde_yaml::Value>,
    hooks: Option<serde_yaml::Value>,
    #[serde(rename = "mcpServers")]
    mcp_servers: Option<serde_yaml::Value>,
    #[serde(rename = "permissionMode")]
    permission_mode: Option<serde_yaml::Value>,
}

fn lower_agent(
    fm: Option<&str>,
    body: &str,
    stem: &str,
    ignored: &mut Vec<IgnoredFeature>,
) -> Result<AgentAsset> {
    let fm: ClaudeAgentFm = match fm {
        Some(s) => serde_yaml::from_str(s)?,
        None => ClaudeAgentFm::default(),
    };
    let name = fm.name.clone().unwrap_or_else(|| stem.to_string());
    let allowed_tools = fm
        .tools
        .as_ref()
        .map(yaml_to_string_list)
        .unwrap_or_default();

    for (present, field) in [
        (fm.disallowed_tools.is_some(), "disallowedTools"),
        (fm.hooks.is_some(), "hooks"),
        (fm.mcp_servers.is_some(), "mcpServers"),
        (fm.permission_mode.is_some(), "permissionMode"),
    ] {
        if present {
            ignored.push(IgnoredFeature {
                kind: "agent-field".to_string(),
                detail: format!("{field} on agent {name}"),
            });
        }
    }

    Ok(AgentAsset {
        name,
        description: fm.description.unwrap_or_default(),
        model: fm.model,
        system_prompt: body.trim().to_string(),
        allowed_tools,
        max_turns: fm.max_turns,
    })
}

fn lower_mcp_servers(
    root: &Path,
    manifest: &ClaudePluginJson,
    draft: &mut CanonicalDraft,
) -> Result<()> {
    // Prefer `.mcp.json`; fall back to an inline `mcpServers` object.
    let servers = if root.join(".mcp.json").is_file() {
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(root.join(".mcp.json"))?)?;
        v.get("mcpServers").and_then(|m| m.as_object()).cloned()
    } else {
        manifest
            .mcp_servers
            .as_ref()
            .and_then(|m| m.as_object())
            .cloned()
    };
    let Some(map) = servers else {
        return Ok(());
    };
    for (name, def) in map {
        if let Some(srv) = lower_mcp_server(&name, &def, &mut draft.ignored) {
            draft.mcp_servers.push(srv);
        }
    }
    Ok(())
}

fn lower_mcp_server(
    name: &str,
    def: &serde_json::Value,
    ignored: &mut Vec<IgnoredFeature>,
) -> Option<McpServerDraft> {
    let obj = def.as_object()?;
    let env: BTreeMap<String, String> = obj
        .get("env")
        .and_then(|e| e.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    if obj.contains_key("cwd") {
        ignored.push(IgnoredFeature {
            kind: "mcp-cwd".to_string(),
            detail: format!("cwd on mcp server {name}"),
        });
    }
    let transport = if let Some(cmd) = obj.get("command").and_then(|c| c.as_str()) {
        let args = obj
            .get("args")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        McpTransport::Stdio {
            command: cmd.to_string(),
            args,
        }
    } else if let Some(url) = obj.get("url").and_then(|u| u.as_str()) {
        match obj.get("type").and_then(|t| t.as_str()) {
            Some("sse") => McpTransport::Sse {
                url: url.to_string(),
            },
            _ => McpTransport::Http {
                url: url.to_string(),
            },
        }
    } else {
        ignored.push(IgnoredFeature {
            kind: "mcp-unparseable".to_string(),
            detail: format!("mcp server {name} has neither command nor url"),
        });
        return None;
    };
    Some(McpServerDraft {
        name: name.to_string(),
        transport,
        env,
    })
}

fn yaml_to_string_list(v: &serde_yaml::Value) -> Vec<String> {
    match v {
        serde_yaml::Value::String(s) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}
