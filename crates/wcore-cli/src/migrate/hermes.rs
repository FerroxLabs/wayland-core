//! Hermes → wayland-core source loader + mappers (issue #228).
//!
//! Reads a Hermes home (`~/.hermes` by default) and turns each
//! `profiles/<name>/` into a wayland-core [`ProfilePlan`]. Pure reconnaissance
//! of the source tree — nothing here writes to disk; the apply step in
//! [`super::apply_plan`] is the only writer.
//!
//! On-disk formats consumed (verified against a real Hermes install):
//!   - `profiles/<name>/config.yaml` — a `model:` block
//!     (`default` / `provider` / `base_url` / `api_mode`) and an optional
//!     `mcp_servers:` map.
//!   - `profiles/<name>/.env` — dotenv, provider-named keys (`<PROVIDER>_API_KEY`).
//!   - `profiles/<name>/{skills/,SOUL.md,memories/}` — counted for the deferred
//!     inventory, not imported in this slice.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use wcore_config::config::{McpServerConfig, ProfileConfig, TransportType};

use wcore_config::portability::ROOT_PROFILE_ID;

use super::{Deferred, MigrationPlan, ProfilePlan};

/// Resolve and validate the Hermes home to import from.
///
/// A home is acceptable when EITHER `profiles/` exists OR a root-level
/// `config.yaml` does. Requiring `profiles/` made a root-only Hermes install
/// unimportable outright, and made a rooted install's own top-level setup
/// invisible — measured against the real install, whose 8.9K root `config.yaml`
/// and root `.env` were both being dropped (F26-01 gap 4).
pub fn detect_home(explicit: Option<&Path>) -> Result<PathBuf> {
    let home = match explicit {
        Some(p) => p.to_path_buf(),
        None => dirs::home_dir()
            .context("cannot resolve the home directory to locate ~/.hermes")?
            .join(".hermes"),
    };
    let profiles = home.join("profiles");
    if !profiles.is_dir() && !root_config_path(&home).is_file() {
        bail!(
            "no Hermes setup found — expected a directory at {} or a root config at {}",
            profiles.display(),
            root_config_path(&home).display()
        );
    }
    Ok(home)
}

/// The root-level `config.yaml` that sits beside `profiles/`.
fn root_config_path(home: &Path) -> PathBuf {
    home.join("config.yaml")
}

/// Walk `<home>/profiles/*` and build the full migration plan.
pub fn build_plan(home: &Path, include_credentials: bool) -> Result<MigrationPlan> {
    let profiles_dir = home.join("profiles");
    let existing_profiles = existing_profile_names();
    let existing_mcp = existing_mcp_names();

    // A root-only home has no `profiles/` at all, and that is now legal — so a
    // missing directory yields an empty set rather than aborting the walk.
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(&profiles_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) if root_config_path(home).is_file() => Vec::new(),
        Err(e) => {
            return Err(anyhow::Error::from(e))
                .with_context(|| format!("reading {}", profiles_dir.display()));
        }
    };
    entries.sort();

    let mut profiles = Vec::new();
    let mut mcp_servers: BTreeMap<String, McpServerConfig> = BTreeMap::new();
    let mut mcp_conflicts: Vec<String> = Vec::new();
    let mut deferred = Deferred::default();
    let mut warnings = Vec::new();

    // The ROOT-level setup, imported alongside the per-profile ones. Before
    // F26-01 this was dropped entirely: `detect_home` required `profiles/`, so
    // a root-only install was unimportable and a rooted install's own top-level
    // `config.yaml` + `.env` were invisible. The root uses the SAME mapper as a
    // profile — measured: the real root `config.yaml` carries exactly the
    // `model:` keys and `mcp_servers:` map the profile mapper already consumes.
    if root_config_path(home).is_file() {
        // A real profile cannot be named this: `ROOT_PROFILE_ID` contains `/`,
        // which is not a legal directory name, so the collision is impossible
        // rather than merely detected. Warn anyway if a profile called `root`
        // exists, so a user who expected that name is told where it went.
        if entries
            .iter()
            .any(|d| d.file_name().and_then(|n| n.to_str()) == Some("root"))
        {
            warnings.push(format!(
                "a profile directory named \"root\" exists; the home's own \
                 top-level setup is imported separately as {ROOT_PROFILE_ID:?}"
            ));
        }
        match map_setup(
            home,
            ROOT_PROFILE_ID.to_string(),
            &root_config_path(home),
            home,
            include_credentials,
            &existing_profiles,
            &existing_mcp,
            &mut mcp_servers,
            &mut mcp_conflicts,
            &mut deferred,
        ) {
            Ok(p) => profiles.push(p),
            Err(e) => warnings.push(format!("root setup: {e}")),
        }
    }

    for dir in &entries {
        let dir = dir.as_path();
        let name = match dir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let cfg_path = dir.join("config.yaml");
        if !cfg_path.is_file() {
            warnings.push(format!("profile {name:?}: no config.yaml — skipped"));
            continue;
        }
        let p = map_setup(
            home,
            name,
            &cfg_path,
            dir,
            include_credentials,
            &existing_profiles,
            &existing_mcp,
            &mut mcp_servers,
            &mut mcp_conflicts,
            &mut deferred,
        )?;
        // A document that PARSES but maps to nothing is the silently-empty
        // result that reads as success. Found by the hostile case
        // `malformed-deepnest`: a 400-level-deep YAML document carrying no key
        // this importer recognises deserialises cleanly into an all-`None`
        // config, and the profile was then reported as imported with no
        // indication that nothing came with it. Named here rather than
        // refused, because an operator whose peer profile genuinely inherits
        // its model from elsewhere is not doing anything wrong — but they must
        // be told, or they discover it later as missing functionality.
        //
        // The relative source path is used deliberately: a machine path in a
        // warning would cross the same boundary 26-01 closed in the emitted
        // plan.
        if p.config.provider.is_none() && p.config.model.is_none() {
            warnings.push(format!(
                "profile {:?}: {}/config.yaml declares neither a provider nor a \
                 model — imported empty",
                p.name, p.source_path
            ));
        }
        profiles.push(p);
    }

    if profiles.is_empty() {
        bail!(
            "no importable Hermes setup under {} (no profiles, no root config.yaml)",
            home.display()
        );
    }

    mcp_conflicts.sort();
    mcp_conflicts.dedup();
    Ok(MigrationPlan {
        source: "hermes",
        source_home: home.to_path_buf(),
        profiles,
        mcp_servers,
        mcp_conflicts,
        deferred,
        deferred_other: BTreeMap::new(),
        warnings,
    })
}

/// Map ONE Hermes setup — a `profiles/<name>/` directory, or the home root —
/// onto a [`ProfilePlan`].
///
/// Extracted so the root-level setup goes through exactly the same mapping as a
/// profile rather than a second, divergent copy of it. `cfg_path` is the
/// `config.yaml` to read and `dir` is the directory whose `.env`, `skills/`,
/// `SOUL.md` and `memories/` belong to this setup.
#[allow(clippy::too_many_arguments)]
fn map_setup(
    home: &Path,
    name: String,
    cfg_path: &Path,
    dir: &Path,
    include_credentials: bool,
    existing_profiles: &HashSet<String>,
    existing_mcp: &HashSet<String>,
    mcp_servers: &mut BTreeMap<String, McpServerConfig>,
    mcp_conflicts: &mut Vec<String>,
    deferred: &mut Deferred,
) -> Result<ProfilePlan> {
    let hermes =
        parse_config(cfg_path).with_context(|| format!("parsing {}", cfg_path.display()))?;

    let (provider, model) = map_model(&hermes.model);
    let mut config = ProfileConfig {
        provider,
        model,
        base_url: hermes.model.base_url.clone(),
        ..Default::default()
    };

    // MCP servers: add new ones globally, reference all by name.
    let mut refs: Vec<String> = Vec::new();
    for (srv_name, srv) in hermes.mcp_servers.unwrap_or_default() {
        if existing_mcp.contains(&srv_name) {
            if !mcp_conflicts.contains(&srv_name) {
                mcp_conflicts.push(srv_name.clone());
            }
        } else {
            mcp_servers
                .entry(srv_name.clone())
                .or_insert_with(|| map_mcp(&srv));
        }
        refs.push(srv_name);
    }
    refs.sort();
    refs.dedup();
    if !refs.is_empty() {
        config.mcp_servers = Some(refs.clone());
    }

    // Credentials (value read only when it may be written; the NAME is always
    // what the preview records).
    let mut credential_env_var = None;
    let mut has_credential = false;
    let mut credential_file = None;
    let env_path = dir.join(".env");
    if env_path.is_file() {
        let env = parse_dotenv(&env_path).unwrap_or_default();
        if let Some((var, value)) = pick_provider_key(&env, config.provider.as_deref()) {
            has_credential = true;
            credential_env_var = Some(var);
            credential_file = Some(relative_to(home, &env_path));
            if include_credentials {
                config.api_key = Some(value);
            }
        }
    }

    // Deferred inventory.
    deferred.skills += count_subdirs(&dir.join("skills"));
    if dir.join("SOUL.md").is_file() {
        deferred.personas += 1;
    }
    deferred.memory_files += count_memory_notes(&dir.join("memories"));

    Ok(ProfilePlan {
        conflict: existing_profiles.contains(&name),
        name,
        config,
        has_credential,
        credential_env_var,
        credential_file,
        mcp_refs: refs,
        source_path: relative_to(home, dir),
    })
}

/// A `/`-separated path relative to the source home, for the emitted plan. An
/// absolute path on the discovering machine must never reach a document.
pub(super) fn relative_to(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

// --- Hermes source schema (permissive; unknown keys ignored) ---

#[derive(Debug, Deserialize, Default)]
struct HermesConfig {
    #[serde(default)]
    model: HermesModel,
    #[serde(default)]
    mcp_servers: Option<HashMap<String, HermesMcpServer>>,
}

#[derive(Debug, Deserialize, Default)]
struct HermesModel {
    default: Option<String>,
    provider: Option<String>,
    base_url: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct HermesMcpServer {
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
    transport: Option<String>,
}

fn parse_config(path: &Path) -> Result<HermesConfig> {
    let raw = std::fs::read_to_string(path)?;
    let cfg: HermesConfig = serde_yaml::from_str(&raw)?;
    Ok(cfg)
}

/// Map the Hermes `model:` block to `(provider, model)`. A leading
/// `<provider>/` prefix on the model id is stripped when it matches the
/// declared provider (Hermes stores e.g. `deepseek/deepseek-v4-pro` +
/// `provider: deepseek`; wayland-core wants the bare `deepseek-v4-pro`).
fn map_model(m: &HermesModel) -> (Option<String>, Option<String>) {
    let provider = m.provider.clone();
    let model = m.default.clone().map(|d| {
        if let Some(p) = &provider
            && let Some(rest) = d.strip_prefix(&format!("{p}/"))
        {
            return rest.to_string();
        }
        d
    });
    (provider, model)
}

fn map_mcp(s: &HermesMcpServer) -> McpServerConfig {
    let transport = match s.transport.as_deref() {
        Some("sse") => TransportType::Sse,
        Some("http" | "streamable-http" | "streamable_http") => TransportType::StreamableHttp,
        Some("stdio") => TransportType::Stdio,
        // No explicit transport: URL-only ⇒ HTTP, otherwise stdio.
        _ if s.url.is_some() && s.command.is_none() => TransportType::StreamableHttp,
        _ => TransportType::Stdio,
    };
    McpServerConfig {
        transport,
        command: s.command.clone(),
        args: s.args.clone(),
        env: s.env.clone(),
        url: s.url.clone(),
        headers: s.headers.clone(),
        deferred: None,
        allow_local: false,
        only_for_assistant: None,
        allowed_tools: None,
    }
}

/// Minimal dotenv reader: `KEY=VALUE`, `#` comments and blank lines skipped, an
/// optional `export ` prefix stripped, and a single layer of matching quotes
/// removed. Good enough for the provider-key case; not a full dotenv parser.
fn parse_dotenv(path: &Path) -> Result<HashMap<String, String>> {
    let raw = std::fs::read_to_string(path)?;
    let mut map = HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut val = val.trim();
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = &val[1..val.len() - 1];
        }
        map.insert(key.to_string(), val.to_string());
    }
    Ok(map)
}

/// Choose the provider API key from a profile's `.env`. Prefers the
/// provider-named `<PROVIDER>_API_KEY`; otherwise falls back to the
/// lexicographically-first `*_API_KEY` so the choice is deterministic.
fn pick_provider_key(
    env: &HashMap<String, String>,
    provider: Option<&str>,
) -> Option<(String, String)> {
    if let Some(p) = provider {
        let want = format!("{}_API_KEY", p.to_ascii_uppercase());
        if let Some(v) = env.get(&want) {
            return Some((want, v.clone()));
        }
    }
    let mut candidates: Vec<(&String, &String)> = env
        .iter()
        .filter(|(k, _)| k.ends_with("_API_KEY"))
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(b.0));
    candidates
        .first()
        .map(|(k, v)| ((*k).clone(), (*v).clone()))
}

fn count_subdirs(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0)
}

/// Count `*.md` memory notes, excluding the `MEMORY.md` entrypoint (mirrors the
/// `wcore-memory` legacy importer's exclusion).
fn count_memory_notes(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension().and_then(|x| x.to_str()) == Some("md")
                        && e.file_name() != "MEMORY.md"
                })
                .count()
        })
        .unwrap_or(0)
}

pub(super) fn existing_profile_names() -> HashSet<String> {
    wcore_config::config::global_profiles()
        .into_iter()
        .map(|(name, _, _)| name)
        .collect()
}

/// Read the `[mcp.servers]` names already present in the global `config.toml`.
/// Best-effort and read-only: any missing file or parse error yields an empty
/// set (the apply step never clobbers an existing server regardless).
pub(super) fn existing_mcp_names() -> HashSet<String> {
    #[derive(Deserialize, Default)]
    struct Probe {
        #[serde(default)]
        mcp: McpProbe,
    }
    #[derive(Deserialize, Default)]
    struct McpProbe {
        #[serde(default)]
        servers: HashMap<String, toml::Value>,
    }

    let path = wcore_config::config::global_config_path();
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    toml::from_str::<Probe>(&raw)
        .map(|p| p.mcp.servers.into_keys().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_model_strips_matching_provider_prefix() {
        let m = HermesModel {
            default: Some("deepseek/deepseek-v4-pro".into()),
            provider: Some("deepseek".into()),
            base_url: Some("https://api.deepseek.com/v1".into()),
        };
        let (provider, model) = map_model(&m);
        assert_eq!(provider.as_deref(), Some("deepseek"));
        assert_eq!(model.as_deref(), Some("deepseek-v4-pro"));
    }

    #[test]
    fn map_model_keeps_id_when_prefix_does_not_match() {
        let m = HermesModel {
            default: Some("anthropic/claude".into()),
            provider: Some("openrouter".into()),
            base_url: None,
        };
        let (provider, model) = map_model(&m);
        assert_eq!(provider.as_deref(), Some("openrouter"));
        // Prefix belongs to a different provider — must NOT be stripped.
        assert_eq!(model.as_deref(), Some("anthropic/claude"));
    }

    #[test]
    fn dotenv_parses_export_quotes_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".env");
        std::fs::write(
            &p,
            "# comment\nexport DEEPSEEK_API_KEY=\"sk-abc\"\nOPENAI_API_KEY=sk-plain\n\nBROKEN\n",
        )
        .unwrap();
        let env = parse_dotenv(&p).unwrap();
        assert_eq!(env.get("DEEPSEEK_API_KEY").unwrap(), "sk-abc");
        assert_eq!(env.get("OPENAI_API_KEY").unwrap(), "sk-plain");
        assert!(!env.contains_key("BROKEN"));
    }

    #[test]
    fn pick_provider_key_prefers_provider_named_var() {
        let mut env = HashMap::new();
        env.insert("OPENAI_API_KEY".to_string(), "openai".to_string());
        env.insert("DEEPSEEK_API_KEY".to_string(), "deepseek".to_string());
        let (var, val) = pick_provider_key(&env, Some("deepseek")).unwrap();
        assert_eq!(var, "DEEPSEEK_API_KEY");
        assert_eq!(val, "deepseek");
    }

    #[test]
    fn pick_provider_key_falls_back_deterministically() {
        let mut env = HashMap::new();
        env.insert("OPENAI_API_KEY".to_string(), "b".to_string());
        env.insert("ANTHROPIC_API_KEY".to_string(), "a".to_string());
        // Provider not present as its own var ⇒ lexicographically-first *_API_KEY.
        let (var, _) = pick_provider_key(&env, Some("deepseek")).unwrap();
        assert_eq!(var, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn map_mcp_infers_transport() {
        let stdio = HermesMcpServer {
            command: Some("srv".into()),
            ..Default::default()
        };
        assert_eq!(map_mcp(&stdio).transport, TransportType::Stdio);

        let http = HermesMcpServer {
            url: Some("https://example.com/mcp".into()),
            ..Default::default()
        };
        assert_eq!(map_mcp(&http).transport, TransportType::StreamableHttp);
    }

    #[test]
    fn parse_config_ignores_unknown_keys_and_null_mcp() {
        let yaml = "model:\n  default: deepseek/x\n  provider: deepseek\n  api_mode: chat_completions\nmcp_servers:\nterminal:\n  foo: bar\n";
        let cfg: HermesConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.model.provider.as_deref(), Some("deepseek"));
        assert!(cfg.mcp_servers.unwrap_or_default().is_empty());
    }
}
