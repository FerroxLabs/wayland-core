//! Codex plugin-format adapter. Lowers a `.codex-plugin/plugin.json` plugin
//! into the same [`CanonicalDraft`] the Claude Code adapter produces, so
//! everything downstream — install plan, consent surface, prompt-risk scan, MCP
//! spawn consent, provenance, commit — is reused unchanged. No second runtime
//! plugin system exists: a lowered Codex plugin becomes an ordinary declarative
//! Wayland plugin directory.
//!
//! # Conformance map (source: openai/codex @ main, fetched 2026-08-27)
//!
//! Schema sources, all first-party:
//! * `codex-rs/skills/src/assets/samples/plugin-creator/references/plugin-json-spec.md`
//!   — the field guide for `plugin.json` and `marketplace.json`.
//! * `codex-rs/core-plugins/src/manifest.rs` — `RawPluginManifest`, the shapes
//!   each field accepts (`string | [string]`, inline object, …).
//! * `codex-rs/core-plugins/src/loader.rs` — the default component locations:
//!   `skills/`, `hooks/hooks.json`, `.mcp.json`, `.app.json`.
//! * `codex-rs/exec-server-protocol/src/protocol.rs` —
//!   `DISCOVERABLE_PLUGIN_MANIFEST_PATHS`, which puts `.codex-plugin/plugin.json`
//!   first.
//! * `codex-rs/codex-mcp/src/plugin_config.rs` + `codex-rs/config/src/mcp_types.rs`
//!   — the MCP file shapes and the per-server fields.
//! * `codex-rs/connectors/src/plugin_config.rs` — `.app.json` (`apps` → `id`).
//! * `codex-rs/config/src/hook_config.rs` — `hooks.json` event names.
//!
//! | Codex `plugin.json` field | Lowered to | Note |
//! |---|---|---|
//! | `name` | `CanonicalDraft::name` | falls back to the marketplace entry name |
//! | `version` | `ResolvedVersion::Explicit` | falls back to the entry version, then `Unknown` |
//! | `skills` (`string \| [string]`) | `SkillAsset[]` | supplements the default `skills/` root, per spec |
//! | `commands` (`string \| [string]`) | `CommandAsset[]` | Codex has no default commands dir |
//! | `mcpServers` (`string` path \| object) | `McpServerDraft[]` | supplements the default `.mcp.json` |
//! | `hooks` (`string \| [string] \| object \| [object]`) | LOSSY `hooks` | Wayland does not run foreign hooks; grade drops to `HooksIgnored` |
//! | `apps` (`string`) / default `.app.json` | LOSSY `apps` | connectors have no Wayland runtime |
//! | `interface.*` | LOSSY `interface` | presentation-only metadata |
//! | `description`, `author`, `homepage`, `repository`, `license`, `keywords` | LOSSY `manifest-metadata` | not carried into `plugin.toml` |
//!
//! Per-server MCP fields with no `McpTransport` equivalent (`cwd`, `env_vars`,
//! `http_headers`, `env_http_headers`, `bearer_token`, `bearer_token_env_var`,
//! `oauth`, tool gating, timeouts, …) are reported as LOSSY `mcp-field`, by NAME
//! only — a `bearer_token` value is never echoed into the consent surface.
//!
//! Every path string above is attacker-controlled and passes through
//! [`crate::path_guard`] before it is joined, stat-ed or read.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use wcore_plugin_api::mcp_server_spec::McpTransport;

use crate::Result;
use crate::adapter::{PluginFormatAdapter, detect_format};
use crate::error::PluginSrcError;
use crate::frontmatter::frontmatter_name;
use crate::model::{
    CanonicalDraft, CommandAsset, IgnoredFeature, McpServerDraft, ResolvedVersion, SkillAsset,
    SourceEntry,
};
use crate::path_guard::resolve_within;

/// `.codex-plugin/plugin.json`, relative to the plugin root.
pub const MANIFEST_RELATIVE_PATH: &str = ".codex-plugin/plugin.json";

/// Codex's default component locations (`loader.rs`).
const DEFAULT_SKILLS_DIR: &str = "skills";
const DEFAULT_HOOKS_FILE: &str = "hooks/hooks.json";
const DEFAULT_MCP_FILE: &str = ".mcp.json";
const DEFAULT_APP_FILE: &str = ".app.json";

/// How deep the skills walk descends. Codex discovers skills recursively; a
/// bound keeps a hostile (or merely deep) tree from running away. Symlinked
/// directories are skipped outright, so this is a depth bound, not a cycle
/// guard.
const MAX_SKILL_DEPTH: usize = 8;

/// MCP server keys Wayland's `McpTransport` can carry. Anything else in a
/// server object is reported as lossy.
const CARRIED_MCP_KEYS: &[&str] = &["command", "args", "env", "url", "type"];

pub struct CodexAdapter;

impl PluginFormatAdapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn detect(&self, root: &Path) -> bool {
        detect_format(root).as_deref() == Some("codex")
    }

    fn lower(&self, marketplace: &str, entry: &SourceEntry, root: &Path) -> Result<CanonicalDraft> {
        let manifest = read_plugin_json(root)?;
        let name = manifest
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| entry.name.clone());
        let mut draft = CanonicalDraft::empty(marketplace, &name);

        draft.version = match manifest
            .version
            .clone()
            .or_else(|| entry.declared_version.clone())
        {
            Some(v) => ResolvedVersion::Explicit(v),
            None => ResolvedVersion::Unknown,
        };

        lower_skills(root, &manifest, &mut draft)?;
        lower_commands(root, &manifest, &mut draft)?;
        lower_mcp_servers(root, &manifest, &mut draft)?;
        report_hooks(root, &manifest, &mut draft)?;
        report_apps(root, &manifest, &mut draft)?;
        report_interface(&manifest, &mut draft);
        report_metadata(&manifest, &mut draft);
        report_unused_surfaces(root, &mut draft);

        // Same prompt-injection / credential-marker scan the Claude Code
        // adapter runs: a Codex plugin's skill and command bodies become part
        // of the agent's instruction context exactly the same way.
        draft.warnings = crate::scan::scan_draft_assets(root, &draft);

        draft.grade = draft.effective_grade();
        Ok(draft)
    }
}

/// Permissive view of `.codex-plugin/plugin.json`. Unknown fields are ignored,
/// matching Codex's own load-tolerant `RawPluginManifest`.
#[derive(Debug, Default, Deserialize)]
struct CodexPluginJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<Value>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    keywords: Option<Value>,
    #[serde(default)]
    skills: Option<Value>,
    #[serde(default)]
    commands: Option<Value>,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Option<Value>,
    #[serde(default)]
    apps: Option<Value>,
    #[serde(default)]
    hooks: Option<Value>,
    #[serde(default)]
    interface: Option<Value>,
}

fn read_plugin_json(root: &Path) -> Result<CodexPluginJson> {
    let p = root.join(MANIFEST_RELATIVE_PATH);
    if !p.is_file() {
        // Codex tolerates a plugin with no manifest and falls back to default
        // component discovery; so do we.
        return Ok(CodexPluginJson::default());
    }
    let txt = fs::read_to_string(&p)?;
    serde_json::from_str(&txt)
        .map_err(|e| PluginSrcError::PluginManifest(format!("{}: {e}", p.display())))
}

/// A manifest value that is `"./x"` or `["./x", "./y"]` becomes a path list.
/// Any other shape yields an empty list; the caller reports it as lossy.
fn path_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

/// Default `skills/` root plus every manifest-declared skills path (the spec is
/// explicit that declared paths SUPPLEMENT default discovery rather than
/// replacing it). Each declared path is traversal-checked and containment-
/// checked before it is walked.
fn lower_skills(root: &Path, manifest: &CodexPluginJson, draft: &mut CanonicalDraft) -> Result<()> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if root.join(DEFAULT_SKILLS_DIR).is_dir() {
        roots.push(PathBuf::from(DEFAULT_SKILLS_DIR));
    }
    for declared in path_list(manifest.skills.as_ref()) {
        let abs = resolve_within(root, &declared)?;
        if !abs.is_dir() {
            continue;
        }
        let rel = PathBuf::from(declared.trim_start_matches("./"));
        if !roots.contains(&rel) {
            roots.push(rel);
        }
    }

    let mut seen: Vec<PathBuf> = Vec::new();
    for rel_root in roots {
        collect_skills(root, &rel_root, 0, &mut seen, draft);
    }
    Ok(())
}

/// Depth-bounded walk for directories holding a `SKILL.md`. Symlinked entries
/// are skipped: the quarantine already normalized the tree, and nothing here
/// re-introduces a link that could point out of it.
fn collect_skills(
    root: &Path,
    rel_dir: &Path,
    depth: usize,
    seen: &mut Vec<PathBuf>,
    draft: &mut CanonicalDraft,
) {
    if depth > MAX_SKILL_DEPTH {
        return;
    }
    let abs_dir = root.join(rel_dir);
    let Ok(entries) = fs::read_dir(&abs_dir) else {
        return;
    };
    for ent in entries.flatten() {
        let Ok(ft) = ent.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let child_rel = rel_dir.join(ent.file_name());
        let child_abs = root.join(&child_rel);
        let skill_md = child_abs.join("SKILL.md");
        if skill_md.is_file() && !seen.contains(&child_rel) {
            let basename = ent.file_name().to_string_lossy().to_string();
            let name = frontmatter_name(&skill_md).unwrap_or_else(|| basename.clone());
            seen.push(child_rel.clone());
            draft.skills.push(SkillAsset {
                name,
                rel_dir: child_rel.clone(),
            });
            // A skill directory is a leaf: its supporting files are copied
            // wholesale, so do not also register nested skills inside it.
            continue;
        }
        collect_skills(root, &child_rel, depth + 1, seen, draft);
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Codex declares commands explicitly (there is no default commands dir). A
/// declared path may name a directory of markdown files or a single `.md`.
fn lower_commands(
    root: &Path,
    manifest: &CodexPluginJson,
    draft: &mut CanonicalDraft,
) -> Result<()> {
    for declared in path_list(manifest.commands.as_ref()) {
        let abs = resolve_within(root, &declared)?;
        let rel = PathBuf::from(declared.trim_start_matches("./"));
        if abs.is_file() {
            push_command(&rel, draft);
        } else if abs.is_dir() {
            let Ok(entries) = fs::read_dir(&abs) else {
                continue;
            };
            let mut files: Vec<_> = entries
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .map(|e| e.file_name())
                .collect();
            files.sort();
            for f in files {
                push_command(&rel.join(f), draft);
            }
        }
    }
    Ok(())
}

fn push_command(rel_file: &Path, draft: &mut CanonicalDraft) {
    if rel_file.extension().and_then(|s| s.to_str()) != Some("md") {
        return;
    }
    let name = rel_file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if draft.commands.iter().any(|c| c.rel_file == rel_file) {
        return;
    }
    draft.commands.push(CommandAsset {
        name,
        rel_file: rel_file.to_path_buf(),
    });
}

// ---------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------

/// Collect servers from the default `.mcp.json` and from `mcpServers` (a path
/// or an inline object), in that order. First declaration of a name wins; a
/// duplicate is reported rather than silently dropped.
fn lower_mcp_servers(
    root: &Path,
    manifest: &CodexPluginJson,
    draft: &mut CanonicalDraft,
) -> Result<()> {
    let mut sources: Vec<serde_json::Map<String, Value>> = Vec::new();

    if root.join(DEFAULT_MCP_FILE).is_file() {
        let txt = fs::read_to_string(root.join(DEFAULT_MCP_FILE))?;
        sources.push(parse_mcp_document(&txt, DEFAULT_MCP_FILE)?);
    }

    match manifest.mcp_servers.as_ref() {
        Some(Value::Object(map)) => sources.push(map.clone()),
        Some(Value::String(rel)) => {
            let abs = resolve_within(root, rel)?;
            if abs.is_file() {
                let txt = fs::read_to_string(&abs)?;
                sources.push(parse_mcp_document(&txt, rel)?);
            }
        }
        Some(other) => draft.ignored.push(IgnoredFeature {
            kind: "mcp-unparseable".to_string(),
            detail: format!(
                "mcpServers is a {}, expected a path string or an object",
                json_type(other)
            ),
        }),
        None => {}
    }

    for map in sources {
        for (name, def) in map {
            if draft.mcp_servers.iter().any(|s| s.name == name) {
                draft.ignored.push(IgnoredFeature {
                    kind: "mcp-duplicate".to_string(),
                    detail: format!("mcp server {name} declared more than once; first kept"),
                });
                continue;
            }
            if let Some(srv) = lower_mcp_server(&name, &def, &mut draft.ignored) {
                draft.mcp_servers.push(srv);
            }
        }
    }
    Ok(())
}

/// Codex accepts BOTH `{"mcpServers": {…}}` and a bare `{name: config}` map
/// (`PluginMcpFile` is untagged over the two). Accept both.
fn parse_mcp_document(txt: &str, label: &str) -> Result<serde_json::Map<String, Value>> {
    let v: Value = serde_json::from_str(txt)
        .map_err(|e| PluginSrcError::PluginManifest(format!("{label}: {e}")))?;
    let Some(obj) = v.as_object() else {
        return Err(PluginSrcError::PluginManifest(format!(
            "{label}: top-level is not an object"
        )));
    };
    match obj.get("mcpServers").and_then(Value::as_object) {
        Some(inner) => Ok(inner.clone()),
        None => Ok(obj.clone()),
    }
}

fn lower_mcp_server(
    name: &str,
    def: &Value,
    ignored: &mut Vec<IgnoredFeature>,
) -> Option<McpServerDraft> {
    let obj = def.as_object()?;

    // Report every key we cannot carry, by NAME only. `bearer_token` and
    // friends are secrets; their values must never reach the consent surface.
    let dropped: Vec<&str> = obj
        .keys()
        .map(String::as_str)
        .filter(|k| !CARRIED_MCP_KEYS.contains(k))
        .collect();
    if !dropped.is_empty() {
        ignored.push(IgnoredFeature {
            kind: "mcp-field".to_string(),
            detail: format!("mcp server {name}: dropped {}", dropped.join(", ")),
        });
    }

    let env: BTreeMap<String, String> = obj
        .get("env")
        .and_then(|e| e.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let declared_type = obj.get("type").and_then(Value::as_str);
    let transport = if let Some(cmd) = obj.get("command").and_then(Value::as_str) {
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
    } else if let Some(url) = obj.get("url").and_then(Value::as_str) {
        match declared_type {
            // Codex's own transport vocabulary is stdio | http |
            // streamable_http | streamable-http; `sse` is accepted here only
            // because a hand-written manifest may still carry it.
            Some("sse") => McpTransport::Sse {
                url: url.to_string(),
            },
            Some("http" | "streamable_http" | "streamable-http") | None => McpTransport::Http {
                url: url.to_string(),
            },
            Some(other) => {
                ignored.push(IgnoredFeature {
                    kind: "mcp-transport".to_string(),
                    detail: format!(
                        "mcp server {name}: unknown transport type '{other}', treated as http"
                    ),
                });
                McpTransport::Http {
                    url: url.to_string(),
                }
            }
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

// ---------------------------------------------------------------------------
// Lossy surfaces
// ---------------------------------------------------------------------------

/// Hooks are never run for a foreign plugin. Report them (which pulls the grade
/// down to `HooksIgnored`) and name the events so the operator can see exactly
/// what will not fire.
fn report_hooks(root: &Path, manifest: &CodexPluginJson, draft: &mut CanonicalDraft) -> Result<()> {
    let mut events: Vec<String> = Vec::new();
    let mut present = false;

    if root.join(DEFAULT_HOOKS_FILE).is_file() {
        present = true;
        if let Ok(txt) = fs::read_to_string(root.join(DEFAULT_HOOKS_FILE)) {
            events.extend(hook_events(&txt));
        }
    }

    match manifest.hooks.as_ref() {
        Some(Value::String(rel)) => {
            present = true;
            let abs = resolve_within(root, rel)?;
            if let Ok(txt) = fs::read_to_string(&abs) {
                events.extend(hook_events(&txt));
            }
        }
        Some(Value::Array(items)) => {
            present = true;
            for item in items {
                match item {
                    Value::String(rel) => {
                        let abs = resolve_within(root, rel)?;
                        if let Ok(txt) = fs::read_to_string(&abs) {
                            events.extend(hook_events(&txt));
                        }
                    }
                    other => events.extend(hook_events_from_value(other)),
                }
            }
        }
        Some(other) => {
            present = true;
            events.extend(hook_events_from_value(other));
        }
        None => {}
    }

    if present {
        events.sort();
        events.dedup();
        let detail = if events.is_empty() {
            "plugin declares hooks (not run for foreign plugins)".to_string()
        } else {
            format!(
                "plugin declares hooks (not run for foreign plugins): {}",
                events.join(", ")
            )
        };
        draft.ignored.push(IgnoredFeature {
            kind: "hooks".to_string(),
            detail,
        });
    }
    Ok(())
}

/// Event names out of a `hooks.json` document (`{"hooks": {"PreToolUse": …}}`).
fn hook_events(txt: &str) -> Vec<String> {
    serde_json::from_str::<Value>(txt)
        .map(|v| hook_events_from_value(&v))
        .unwrap_or_default()
}

fn hook_events_from_value(v: &Value) -> Vec<String> {
    let Some(obj) = v.as_object() else {
        return Vec::new();
    };
    let events = match obj.get("hooks").and_then(Value::as_object) {
        Some(inner) => inner,
        None => obj,
    };
    events
        .keys()
        .filter(|k| k.as_str() != "description" && k.as_str() != "state")
        .cloned()
        .collect()
}

/// Codex apps declare CONNECTORS (`{"apps": {"<name>": {"id": "<connector>"}}}`).
/// Wayland has no connector runtime, so every one is reported.
fn report_apps(root: &Path, manifest: &CodexPluginJson, draft: &mut CanonicalDraft) -> Result<()> {
    let mut docs: Vec<String> = Vec::new();
    // Declared, not parsed: like `report_hooks`, the operator-facing fact is
    // "this will not run here", and that is true of a document Wayland cannot
    // read as much as of one it can.
    let mut declared = false;
    let default_doc = root.join(DEFAULT_APP_FILE);
    if default_doc.is_file() {
        declared = true;
        docs.push(fs::read_to_string(&default_doc)?);
    }
    match manifest.apps.as_ref() {
        Some(Value::String(rel)) => {
            declared = true;
            let abs = resolve_within(root, rel)?;
            // `.app.json` is both the default discovery path and a legal value
            // for this field. Reading it through both routes would list every
            // connector twice.
            if abs.is_file() && abs != default_doc {
                docs.push(fs::read_to_string(&abs)?);
            }
        }
        Some(other) => {
            draft.ignored.push(IgnoredFeature {
                kind: "apps".to_string(),
                detail: format!("apps is a {}, expected a path string", json_type(other)),
            });
            return Ok(());
        }
        None => {}
    }

    let mut connectors: Vec<String> = Vec::new();
    for doc in &docs {
        let Ok(v) = serde_json::from_str::<Value>(doc) else {
            continue;
        };
        let Some(apps) = v.get("apps").and_then(Value::as_object) else {
            continue;
        };
        for (app_name, cfg) in apps {
            let id = cfg
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<no id>")
                .to_string();
            connectors.push(format!("{app_name} → {id}"));
        }
    }
    if !connectors.is_empty() {
        connectors.sort();
        connectors.dedup();
        draft.ignored.push(IgnoredFeature {
            kind: "apps".to_string(),
            detail: format!(
                "app connectors have no Wayland runtime: {}",
                connectors.join(", ")
            ),
        });
    } else if declared {
        draft.ignored.push(IgnoredFeature {
            kind: "apps".to_string(),
            detail: "plugin declares apps, which have no Wayland connector runtime \
                     (the declaration could not be read as connectors)"
                .to_string(),
        });
    }
    Ok(())
}

/// `interface` is presentation metadata for the Codex plugin directory. None of
/// it survives into a Wayland `plugin.toml`.
fn report_interface(manifest: &CodexPluginJson, draft: &mut CanonicalDraft) {
    let Some(Value::Object(obj)) = manifest.interface.as_ref() else {
        return;
    };
    if obj.is_empty() {
        return;
    }
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    draft.ignored.push(IgnoredFeature {
        kind: "interface".to_string(),
        detail: format!("presentation metadata not carried: {}", keys.join(", ")),
    });
}

/// Manifest metadata that the generated `plugin.toml` does not carry. Reported
/// as one line so the consent surface stays readable while still never
/// claiming parity it does not have.
fn report_metadata(manifest: &CodexPluginJson, draft: &mut CanonicalDraft) {
    let present: Vec<&str> = [
        ("description", manifest.description.is_some()),
        ("author", manifest.author.is_some()),
        ("homepage", manifest.homepage.is_some()),
        ("repository", manifest.repository.is_some()),
        ("license", manifest.license.is_some()),
        ("keywords", manifest.keywords.is_some()),
    ]
    .into_iter()
    .filter_map(|(k, present)| present.then_some(k))
    .collect();
    if present.is_empty() {
        return;
    }
    draft.ignored.push(IgnoredFeature {
        kind: "manifest-metadata".to_string(),
        detail: format!("not carried into plugin.toml: {}", present.join(", ")),
    });
}

/// Surfaces present in the fetched tree that this adapter deliberately does NOT
/// read.
///
/// `detect_format` prefers `.codex-plugin/plugin.json`, so a plugin shipping
/// both vendor manifests lowers through here — and its `.claude-plugin`
/// manifest, and any `agents/` directory (a Claude Code concept with no Codex
/// equivalent), go unread. Reporting them is the whole point of the exercise:
/// choosing an adapter must never quietly discard what the other one would
/// have picked up.
fn report_unused_surfaces(root: &Path, draft: &mut CanonicalDraft) {
    if root.join(".claude-plugin/plugin.json").is_file() {
        draft.ignored.push(IgnoredFeature {
            kind: "foreign-manifest".to_string(),
            detail: "a .claude-plugin/plugin.json is also present; the Codex manifest was \
                     used and the Claude Code one was not read"
                .to_string(),
        });
    }
    if root.join("agents").is_dir() {
        draft.ignored.push(IgnoredFeature {
            kind: "agents".to_string(),
            detail: "agents/ is not a Codex plugin surface and was not lowered".to_string(),
        });
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn w(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn entry(name: &str) -> SourceEntry {
        SourceEntry {
            name: name.to_string(),
            kind: crate::model::SourceKind::RelativePath(format!("./{name}").into()),
            strict: true,
            declared_version: None,
            description: None,
            unsupported: Vec::new(),
        }
    }

    #[test]
    fn manifest_declared_skills_path_with_dotdot_is_rejected() {
        let d = tempdir().unwrap();
        w(
            &d.path().join(MANIFEST_RELATIVE_PATH),
            r#"{"name":"p","skills":"../../etc"}"#,
        );
        let e = CodexAdapter.lower("m", &entry("p"), d.path()).unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn manifest_declared_mcp_path_with_dotdot_is_rejected() {
        let d = tempdir().unwrap();
        w(
            &d.path().join(MANIFEST_RELATIVE_PATH),
            r#"{"name":"p","mcpServers":"../../secrets.json"}"#,
        );
        let e = CodexAdapter.lower("m", &entry("p"), d.path()).unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn manifest_declared_apps_path_with_dotdot_is_rejected() {
        let d = tempdir().unwrap();
        w(
            &d.path().join(MANIFEST_RELATIVE_PATH),
            r#"{"name":"p","apps":"../../.app.json"}"#,
        );
        let e = CodexAdapter.lower("m", &entry("p"), d.path()).unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn manifest_declared_hooks_path_with_dotdot_is_rejected() {
        let d = tempdir().unwrap();
        w(
            &d.path().join(MANIFEST_RELATIVE_PATH),
            r#"{"name":"p","hooks":"../../hooks.json"}"#,
        );
        let e = CodexAdapter.lower("m", &entry("p"), d.path()).unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn manifest_declared_commands_path_with_dotdot_is_rejected() {
        let d = tempdir().unwrap();
        w(
            &d.path().join(MANIFEST_RELATIVE_PATH),
            r#"{"name":"p","commands":["./ok","../../etc"]}"#,
        );
        let e = CodexAdapter.lower("m", &entry("p"), d.path()).unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn absolute_manifest_path_is_rejected() {
        // `Path::join` replaces its base on an absolute argument.
        let d = tempdir().unwrap();
        #[cfg(unix)]
        let body = r#"{"name":"p","skills":"/etc"}"#;
        #[cfg(windows)]
        let body = r#"{"name":"p","skills":"C:\\Windows"}"#;
        w(&d.path().join(MANIFEST_RELATIVE_PATH), body);
        let e = CodexAdapter.lower("m", &entry("p"), d.path()).unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn a_dual_manifest_tree_reports_what_this_adapter_did_not_read() {
        let d = tempdir().unwrap();
        w(&d.path().join(MANIFEST_RELATIVE_PATH), r#"{"name":"p"}"#);
        w(
            &d.path().join(".claude-plugin/plugin.json"),
            r#"{"name":"p"}"#,
        );
        w(
            &d.path().join("agents/helper.md"),
            "---\nname: helper\n---\nbody",
        );
        w(&d.path().join("skills/a/SKILL.md"), "---\nname: a\n---\nx");

        let draft = CodexAdapter.lower("m", &entry("p"), d.path()).unwrap();
        let kinds: Vec<&str> = draft.ignored.iter().map(|i| i.kind.as_str()).collect();
        assert!(
            kinds.contains(&"foreign-manifest"),
            "the unread Claude Code manifest must be reported: {kinds:?}"
        );
        assert!(
            kinds.contains(&"agents"),
            "the unread agents/ dir must be reported: {kinds:?}"
        );
        // Polarity: it still lowered the Codex surfaces.
        assert_eq!(draft.skills.len(), 1);
    }

    #[test]
    fn clean_manifest_paths_are_accepted() {
        // Polarity control: the guards above reject traversal, not manifest
        // paths as such.
        let d = tempdir().unwrap();
        w(
            &d.path().join(MANIFEST_RELATIVE_PATH),
            r#"{"name":"p","skills":"./extra-skills"}"#,
        );
        w(
            &d.path().join("extra-skills/deploy/SKILL.md"),
            "---\nname: deploy\n---\nbody",
        );
        let draft = CodexAdapter.lower("m", &entry("p"), d.path()).unwrap();
        assert_eq!(draft.skills.len(), 1);
        assert_eq!(draft.skills[0].name, "deploy");
    }
}
