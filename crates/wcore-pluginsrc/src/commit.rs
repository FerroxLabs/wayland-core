//! Transactional commit of a lowered plugin into the Wayland-native store as a
//! **self-contained directory** the runtime discovers unchanged:
//!
//! ```text
//! ~/.wayland/plugins/<plugin>@<marketplace>/
//!   plugin.toml          # declarative manifest (runtime kind = "declarative")
//!   skills/<name>/SKILL.md
//!   commands/<name>.md
//!   agents/<name>.yaml   # converted from the foreign agent
//!   provenance.json      # origin marketplace/source/sha/grade (sidecar)
//! ```
//!
//! Discovery scans one directory level and skips symlinks, so the install dir is
//! a real, flat directory. The skills/agents loaders are taught (Lane D) to scan
//! `plugins/*/skills` and `plugins/*/agents`. v1 emits a single `[mcp_server]`;
//! any additional servers are already recorded as IgnoredFeatures on the draft.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wcore_plugin_api::agent_manifest::AgentManifest;
use wcore_plugin_api::mcp_server_spec::McpTransport;

use crate::Result;
use crate::model::{CanonicalDraft, ResolvedVersion};

/// Origin metadata not derivable from the draft itself.
pub struct CommitMeta<'a> {
    pub marketplace: &'a str,
    pub format: &'a str, // adapter id, e.g. "claude-code"
    pub resolved_sha: Option<String>,
}

/// Audit sidecar written next to the generated `plugin.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub marketplace: String,
    pub plugin: String,
    pub namespace: String,
    pub version: String,
    pub grade: String,
    pub format: String,
    pub resolved_sha: Option<String>,
}

/// Commit a draft into `store_root`, returning the final install directory.
/// Transactional: writes to a staging dir, then atomically renames into place.
/// Re-installing the same plugin replaces the existing directory.
pub fn commit_plan(
    draft: &CanonicalDraft,
    meta: &CommitMeta<'_>,
    fetched_root: &Path,
    store_root: &Path,
) -> Result<PathBuf> {
    let dirname = sanitize(&format!("{}@{}", draft.name, meta.marketplace));
    let final_dir = store_root.join(&dirname);
    let staging = store_root.join(format!(".staging-{dirname}"));

    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;

    // 1. plugin.toml (declarative).
    fs::write(staging.join("plugin.toml"), generate_plugin_toml(draft)?)?;

    // 2. Skills — copy each skill directory verbatim.
    for s in &draft.skills {
        let src = fetched_root.join(&s.rel_dir);
        let dst = staging.join(&s.rel_dir);
        copy_dir_recursive(&src, &dst)?;
    }

    // 3. Commands — copy each flat markdown file verbatim.
    for c in &draft.commands {
        let src = fetched_root.join(&c.rel_file);
        let dst = staging.join(&c.rel_file);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&src, &dst)?;
    }

    // 4. Agents — serialize the converted AgentManifest to YAML.
    if !draft.agents.is_empty() {
        fs::create_dir_all(staging.join("agents"))?;
        for a in &draft.agents {
            let manifest = AgentManifest {
                name: a.name.clone(),
                description: a.description.clone(),
                model: a.model.clone(),
                system_prompt: a.system_prompt.clone(),
                allowed_tools: a.allowed_tools.clone(),
                max_turns: a.max_turns,
            };
            let yaml = serde_yaml::to_string(&manifest)?;
            fs::write(
                staging.join("agents").join(format!("{}.yaml", a.name)),
                yaml,
            )?;
        }
    }

    // 5. Provenance sidecar.
    let provenance = Provenance {
        marketplace: meta.marketplace.to_string(),
        plugin: draft.name.clone(),
        namespace: draft.namespace.clone(),
        version: version_string(&draft.version),
        grade: format!("{:?}", draft.effective_grade()),
        format: meta.format.to_string(),
        resolved_sha: meta.resolved_sha.clone(),
    };
    fs::write(
        staging.join("provenance.json"),
        serde_json::to_string_pretty(&provenance)?,
    )?;

    // 6. Atomic swap: remove any prior install, then rename staging into place.
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir)?;
    }
    fs::rename(&staging, &final_dir)?;
    Ok(final_dir)
}

/// Build the declarative `plugin.toml`. Hand-built via `toml::Table` so only
/// present keys are emitted (avoids `Option`-serialization pitfalls) and the
/// output round-trips through `PluginManifest::from_toml_str`.
fn generate_plugin_toml(draft: &CanonicalDraft) -> Result<String> {
    use toml::Value;

    let mut plugin = toml::Table::new();
    plugin.insert("name".into(), Value::String(draft.name.clone()));
    plugin.insert(
        "version".into(),
        Value::String(version_string(&draft.version)),
    );
    plugin.insert(
        "description".into(),
        Value::String(format!(
            "Installed from marketplace plugin {}",
            draft.namespace
        )),
    );
    plugin.insert("license".into(), Value::String("UNKNOWN".into()));

    let mut perms = toml::Table::new();
    if !draft.skills.is_empty() || !draft.commands.is_empty() {
        perms.insert("register_skills".into(), Value::Boolean(true));
    }
    if !draft.agents.is_empty() {
        perms.insert("register_agents".into(), Value::Boolean(true));
    }
    if !draft.mcp_servers.is_empty() {
        perms.insert("register_mcp_server".into(), Value::Boolean(true));
    }

    let mut runtime = toml::Table::new();
    runtime.insert("kind".into(), Value::String("declarative".into()));

    let mut root = toml::Table::new();
    root.insert("plugin".into(), Value::Table(plugin));
    root.insert("permissions".into(), Value::Table(perms));
    root.insert("runtime".into(), Value::Table(runtime));

    // v1: a single MCP server. Extras are already on draft.ignored.
    if let Some(server) = draft.mcp_servers.first() {
        let mut srv = toml::Table::new();
        srv.insert("name".into(), Value::String(server.name.clone()));

        let mut transport = toml::Table::new();
        match &server.transport {
            McpTransport::Stdio { command, args } => {
                transport.insert("kind".into(), Value::String("stdio".into()));
                transport.insert("command".into(), Value::String(command.clone()));
                transport.insert(
                    "args".into(),
                    Value::Array(args.iter().cloned().map(Value::String).collect()),
                );
            }
            McpTransport::Sse { url } => {
                transport.insert("kind".into(), Value::String("sse".into()));
                transport.insert("url".into(), Value::String(url.clone()));
            }
            McpTransport::Http { url } => {
                transport.insert("kind".into(), Value::String("http".into()));
                transport.insert("url".into(), Value::String(url.clone()));
            }
        }
        srv.insert("transport".into(), Value::Table(transport));

        if !server.env.is_empty() {
            let mut env = toml::Table::new();
            for (k, v) in &server.env {
                env.insert(k.clone(), Value::String(v.clone()));
            }
            srv.insert("env".into(), Value::Table(env));
        }
        root.insert("mcp_server".into(), Value::Table(srv));
    }

    Ok(toml::to_string(&Value::Table(root))?)
}

fn version_string(v: &ResolvedVersion) -> String {
    match v {
        ResolvedVersion::Explicit(s) => s.clone(),
        ResolvedVersion::CommitSha(s) => s.clone(),
        ResolvedVersion::Unknown => "0.0.0".to_string(),
    }
}

/// Sanitize a directory name: keep `[A-Za-z0-9._@-]`, replace the rest with `-`.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '@' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)?.flatten() {
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            fs::copy(&from, &to)?;
        }
        // Symlinks are intentionally skipped (security: the quarantine already
        // dropped escaping symlinks; nothing here re-introduces them).
    }
    Ok(())
}
