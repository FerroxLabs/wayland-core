//! v0.7.0 Task 3.B.1: user-driven agent factory.
//!
//! Builds an `AgentManifest` from declarative inputs and persists it to
//! `~/.wayland/agents/<name>.toml`. The CLI in 3.B.2 and the interactive
//! slash command in 3.B.3 both call into here so persistence semantics
//! stay in one place.

use std::path::{Path, PathBuf};

use wcore_plugin_api::AgentManifest;

use crate::AgentPack;

#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    #[error("agent name must be a non-empty kebab-case slug (got {name:?})")]
    InvalidName { name: String },
    #[error("inherited agent {parent:?} is not a built-in (run --list-agents)")]
    UnknownParent { parent: String },
    #[error("could not resolve user agent directory (HOME unset?)")]
    NoHomeDir,
    #[error("failed to write manifest at {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read manifest at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialise manifest: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("failed to parse manifest: {0}")]
    Deserialize(#[from] toml::de::Error),
}

/// Declarative input for building a manifest.
///
/// `inherit_from` looks the parent up in [`AgentPack`] and uses its prompt /
/// model / max_turns as defaults. Anything specified on `FactoryInput`
/// overrides the parent. Tools are concatenated and deduplicated.
#[derive(Debug, Clone, Default)]
pub struct FactoryInput {
    pub name: String,
    pub description: Option<String>,
    pub inherit_from: Option<String>,
    pub system_prompt: Option<String>,
    pub model: Option<String>,
    pub max_turns: Option<u32>,
    pub extra_allowed_tools: Vec<String>,
}

/// Build an `AgentManifest` from declarative input (without writing it).
pub fn build(input: &FactoryInput) -> Result<AgentManifest, FactoryError> {
    validate_name(&input.name)?;

    let parent = input
        .inherit_from
        .as_deref()
        .map(|p| {
            AgentPack::get(p).ok_or_else(|| FactoryError::UnknownParent {
                parent: p.to_string(),
            })
        })
        .transpose()?;

    let description = input
        .description
        .clone()
        .or_else(|| parent.as_ref().map(|p| p.description.clone()))
        .unwrap_or_else(|| format!("User-defined agent: {}", input.name));

    let system_prompt = input
        .system_prompt
        .clone()
        .or_else(|| parent.as_ref().map(|p| p.system_prompt.clone()))
        .unwrap_or_else(|| "You are an assistant. Be terse and direct. No filler.".to_string());

    let model = input
        .model
        .clone()
        .or_else(|| parent.as_ref().and_then(|p| p.model.clone()));

    let max_turns = input
        .max_turns
        .or_else(|| parent.as_ref().and_then(|p| p.max_turns));

    let mut allowed_tools: Vec<String> = parent
        .as_ref()
        .map(|p| p.allowed_tools.clone())
        .unwrap_or_default();
    for tool in &input.extra_allowed_tools {
        if !allowed_tools.iter().any(|t| t == tool) {
            allowed_tools.push(tool.clone());
        }
    }

    Ok(AgentManifest {
        name: input.name.clone(),
        description,
        model,
        system_prompt,
        allowed_tools,
        max_turns,
    })
}

/// Resolve the user-agent directory (`~/.wayland/agents/`).
pub fn user_agent_dir() -> Result<PathBuf, FactoryError> {
    dirs::home_dir()
        .map(|h| h.join(".wayland").join("agents"))
        .ok_or(FactoryError::NoHomeDir)
}

/// Build a manifest, then persist it to `<base_dir>/<name>.toml`.
/// `base_dir` is normally `user_agent_dir()`; tests inject a tempdir.
pub fn create(input: &FactoryInput, base_dir: &Path) -> Result<PathBuf, FactoryError> {
    let manifest = build(input)?;
    let path = base_dir.join(format!("{}.toml", manifest.name));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| FactoryError::Write {
            path: path.clone(),
            source: e,
        })?;
    }
    let toml = toml::to_string_pretty(&manifest)?;
    std::fs::write(&path, toml).map_err(|e| FactoryError::Write {
        path: path.clone(),
        source: e,
    })?;
    Ok(path)
}

/// Load a previously-persisted manifest from disk.
pub fn load(base_dir: &Path, name: &str) -> Result<AgentManifest, FactoryError> {
    validate_name(name)?;
    let path = base_dir.join(format!("{name}.toml"));
    let raw = std::fs::read_to_string(&path).map_err(|e| FactoryError::Read {
        path: path.clone(),
        source: e,
    })?;
    let manifest: AgentManifest = toml::from_str(&raw)?;
    Ok(manifest)
}

/// List every persisted user agent under `base_dir`.
pub fn list(base_dir: &Path) -> Result<Vec<AgentManifest>, FactoryError> {
    let mut out = Vec::new();
    let read = match std::fs::read_dir(base_dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(FactoryError::Read {
                path: base_dir.to_path_buf(),
                source: e,
            });
        }
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| FactoryError::Read {
            path: path.clone(),
            source: e,
        })?;
        let m: AgentManifest = toml::from_str(&raw)?;
        out.push(m);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Delete a persisted user agent. Returns true if a file was removed.
pub fn delete(base_dir: &Path, name: &str) -> Result<bool, FactoryError> {
    validate_name(name)?;
    let path = base_dir.join(format!("{name}.toml"));
    match std::fs::remove_file(&path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(FactoryError::Write { path, source: e }),
    }
}

fn validate_name(name: &str) -> Result<(), FactoryError> {
    if name.is_empty() {
        return Err(FactoryError::InvalidName {
            name: name.to_string(),
        });
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if !ok {
        return Err(FactoryError::InvalidName {
            name: name.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_from_minimal_input() {
        let input = FactoryInput {
            name: "scratch".to_string(),
            ..Default::default()
        };
        let m = build(&input).expect("minimal build");
        assert_eq!(m.name, "scratch");
        assert!(!m.system_prompt.is_empty());
        assert!(m.allowed_tools.is_empty());
    }

    #[test]
    fn inherits_from_built_in() {
        let input = FactoryInput {
            name: "my-architect".to_string(),
            inherit_from: Some("architect".to_string()),
            ..Default::default()
        };
        let m = build(&input).expect("inherit");
        let parent = AgentPack::get("architect").unwrap();
        assert_eq!(m.system_prompt, parent.system_prompt);
        assert_eq!(m.allowed_tools, parent.allowed_tools);
        assert_eq!(m.max_turns, parent.max_turns);
    }

    #[test]
    fn extra_tools_are_added_and_deduped() {
        let input = FactoryInput {
            name: "my-debugger".to_string(),
            inherit_from: Some("debugger".to_string()),
            extra_allowed_tools: vec![
                "WebFetch".to_string(),
                "Read".to_string(), // debugger already has Read
            ],
            ..Default::default()
        };
        let m = build(&input).expect("extra tools");
        assert_eq!(m.allowed_tools.iter().filter(|t| *t == "Read").count(), 1);
        assert!(m.allowed_tools.iter().any(|t| t == "WebFetch"));
    }

    #[test]
    fn rejects_invalid_names() {
        for bad in &["", "Foo", "-bad", "bad-", "with_underscore", "with space"] {
            let input = FactoryInput {
                name: bad.to_string(),
                ..Default::default()
            };
            assert!(
                matches!(build(&input), Err(FactoryError::InvalidName { .. })),
                "expected InvalidName for {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_unknown_parent() {
        let input = FactoryInput {
            name: "rooted".to_string(),
            inherit_from: Some("not-a-real-agent".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            build(&input),
            Err(FactoryError::UnknownParent { .. })
        ));
    }

    #[test]
    fn round_trips_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let input = FactoryInput {
            name: "round-trip".to_string(),
            description: Some("test agent".to_string()),
            system_prompt: Some("Be terse.".to_string()),
            extra_allowed_tools: vec!["Read".to_string(), "Bash".to_string()],
            max_turns: Some(7),
            ..Default::default()
        };
        let path = create(&input, tmp.path()).expect("create");
        assert!(path.exists());

        let loaded = load(tmp.path(), "round-trip").expect("load");
        assert_eq!(loaded.name, "round-trip");
        assert_eq!(loaded.description, "test agent");
        assert_eq!(loaded.system_prompt, "Be terse.");
        assert_eq!(loaded.allowed_tools, vec!["Read", "Bash"]);
        assert_eq!(loaded.max_turns, Some(7));

        let listed = list(tmp.path()).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "round-trip");

        assert!(delete(tmp.path(), "round-trip").unwrap());
        assert!(!delete(tmp.path(), "round-trip").unwrap());
        assert!(list(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn list_handles_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let listed = list(&missing).expect("list missing");
        assert!(listed.is_empty());
    }
}
