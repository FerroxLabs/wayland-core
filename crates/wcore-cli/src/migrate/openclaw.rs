//! OpenClaw → wayland-core source loader + mappers (F26-01).
//!
//! The reciprocal of [`super::hermes`]: the second supported import source, so
//! a user arriving from either tool has a path in.
//!
//! Pure reconnaissance — nothing here writes. Same idiom as the Hermes source:
//! permissive deserialization that ignores unknown keys, a deterministic total
//! order, and per-entry warnings rather than a hard failure that discards the
//! whole plan.
//!
//! # On-disk format
//!
//! GROUNDED in the peer's own source (`src/config/paths.ts`), reconciled across
//! the pinned baseline `11a0ad10` (2026.6.2), checkout HEAD `3659c85e`
//! (2026.7.2) and the real install. All four path constants are byte-identical
//! at both refs, so there is a single format to target:
//!
//! ```text
//! const LEGACY_STATE_DIRNAMES   = [".clawdbot"]
//! const NEW_STATE_DIRNAME       = ".openclaw"
//! const CONFIG_FILENAME         = "openclaw.json"
//! const LEGACY_CONFIG_FILENAMES = ["clawdbot.json"]
//! ```
//!
//! Home resolution is `OPENCLAW_CONFIG_DIR`/`OPENCLAW_STATE_DIR` if set, else
//! `$HOME/.openclaw`, else the legacy `$HOME/.clawdbot`. **There is no
//! platform-specific branch in the peer** — the same `os.homedir()`-relative
//! resolution is used everywhere — so this importer does not invent one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use wcore_config::config::{McpServerConfig, ProfileConfig, TransportType};
use wcore_config::portability::OPENCLAW_ROOT_PROFILE_ID;

use super::hermes::relative_to;
use super::{Deferred, MigrationPlan, ProfilePlan};

/// The primary config file name, and the legacy name still honoured.
const CONFIG_FILENAME: &str = "openclaw.json";
const LEGACY_CONFIG_FILENAMES: &[&str] = &["clawdbot.json"];
const STATE_DIRNAME: &str = ".openclaw";
const LEGACY_STATE_DIRNAMES: &[&str] = &[".clawdbot"];

/// Trees counted for the deferred inventory. Every one of these exists in the
/// real install and carries user state this slice does not import; counting
/// them is what keeps a discovered item from being silently dropped.
const DEFERRED_DIRS: &[&str] = &[
    "agents",
    "flows",
    "identity",
    "logs",
    "memory",
    "plugin-skills",
    "plugins",
    "tasks",
    "tui",
    "workspace",
];

/// Resolve the OpenClaw home.
///
/// Explicit override first; otherwise the resolution DETERMINED from peer
/// source. Never re-derived or guessed here.
pub fn detect_home(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        let home = p.to_path_buf();
        if config_path(&home).is_none() {
            bail!(
                "no OpenClaw config found under {} — expected {} or one of {:?}",
                home.display(),
                CONFIG_FILENAME,
                LEGACY_CONFIG_FILENAMES
            );
        }
        return Ok(home);
    }

    for var in ["OPENCLAW_CONFIG_DIR", "OPENCLAW_STATE_DIR"] {
        if let Ok(v) = std::env::var(var) {
            let t = v.trim();
            if !t.is_empty() {
                let home = PathBuf::from(t);
                if config_path(&home).is_some() {
                    return Ok(home);
                }
            }
        }
    }

    let base =
        dirs::home_dir().context("cannot resolve the home directory to locate ~/.openclaw")?;
    let mut candidates = vec![base.join(STATE_DIRNAME)];
    candidates.extend(LEGACY_STATE_DIRNAMES.iter().map(|d| base.join(d)));
    for home in &candidates {
        if config_path(home).is_some() {
            return Ok(home.clone());
        }
    }
    bail!(
        "no OpenClaw setup found — looked for {} in {:?}",
        CONFIG_FILENAME,
        candidates
    )
}

/// The config document to read, primary name first then the legacy names.
///
/// Backup and last-known-good SIBLINGS (`openclaw.json.bak*`,
/// `openclaw.json.last-good`) are deliberately NOT candidates: they are prior
/// revisions of the same document, not additional sources. The real install
/// carries EIGHT of them, so treating them as sources would multiply every
/// discovered item nine-fold.
fn config_path(home: &Path) -> Option<PathBuf> {
    let primary = home.join(CONFIG_FILENAME);
    if primary.is_file() {
        return Some(primary);
    }
    LEGACY_CONFIG_FILENAMES
        .iter()
        .map(|n| home.join(n))
        .find(|p| p.is_file())
}

/// Build the migration plan from an OpenClaw home.
pub fn build_plan(home: &Path, include_credentials: bool) -> Result<MigrationPlan> {
    let cfg_path = config_path(home)
        .with_context(|| format!("no OpenClaw config under {}", home.display()))?;
    let rel_cfg = relative_to(home, &cfg_path);

    let raw = std::fs::read_to_string(&cfg_path)
        .with_context(|| format!("reading {}", cfg_path.display()))?;
    let mut warnings: Vec<String> = Vec::new();

    // A malformed primary config is a NAMED error, never a panic and never a
    // silently empty plan that would read as success.
    let cfg: OpenClawConfig = match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(e) => bail!("{} is not valid OpenClaw JSON: {e}", cfg_path.display()),
    };
    if cfg_path.file_name().and_then(|n| n.to_str()) != Some(CONFIG_FILENAME) {
        warnings.push(format!("using legacy config name {rel_cfg:?}"));
    }

    let existing_profiles = super::hermes::existing_profile_names();
    let existing_mcp = super::hermes::existing_mcp_names();

    let mut profiles: Vec<ProfilePlan> = Vec::new();
    let mut mcp_servers: BTreeMap<String, McpServerConfig> = BTreeMap::new();
    let mut mcp_conflicts: Vec<String> = Vec::new();

    // Gateway / channel tokens live on the ROOT document. Read BEFORE the
    // `agents.defaults` move below, and recorded by reference only: even with
    // `--include-credentials` the root carries no provider api key, because its
    // credential is a gateway/channel token rather than a `ProfileConfig::api_key`.
    let root_cred = first_root_credential(&cfg);

    // --- the root setup: agents.defaults ---
    let defaults = cfg.agents.defaults.clone().unwrap_or_default();
    let primary = defaults.model.and_then(|m| m.primary);
    let (root_provider, root_model) = split_qualified(primary.as_deref());
    let root_base_url = root_provider
        .as_deref()
        .and_then(|p| cfg.models.providers.get(p))
        .and_then(|p| p.base_url.clone());

    // --- MCP servers ---
    for (name, srv) in &cfg.mcp.servers {
        if existing_mcp.contains(name) {
            if !mcp_conflicts.contains(name) {
                mcp_conflicts.push(name.clone());
            }
        } else {
            mcp_servers
                .entry(name.clone())
                .or_insert_with(|| map_mcp(srv));
        }
    }
    let mut root_refs: Vec<String> = cfg.mcp.servers.keys().cloned().collect();
    root_refs.sort();
    root_refs.dedup();

    let mut root_cfg = ProfileConfig {
        provider: root_provider.clone(),
        model: root_model,
        base_url: root_base_url,
        ..Default::default()
    };
    if !root_refs.is_empty() {
        root_cfg.mcp_servers = Some(root_refs.clone());
    }
    profiles.push(ProfilePlan {
        conflict: existing_profiles.contains(OPENCLAW_ROOT_PROFILE_ID),
        name: OPENCLAW_ROOT_PROFILE_ID.to_string(),
        config: root_cfg,
        has_credential: root_cred.is_some(),
        credential_env_var: root_cred.clone(),
        credential_file: root_cred.as_ref().map(|_| rel_cfg.clone()),
        mcp_refs: root_refs,
        source_path: rel_cfg.clone(),
    });

    // --- one profile per configured provider ---
    for (name, prov) in &cfg.models.providers {
        let mut config = ProfileConfig {
            provider: Some(name.clone()),
            model: prov.models.first().and_then(|m| m.id.clone()),
            base_url: prov.base_url.clone(),
            ..Default::default()
        };
        // A provider's api key: the NAME of the field is recorded, the value is
        // read only when the caller opted in, exactly as the Hermes source does.
        let has_key = prov
            .api_key
            .as_deref()
            .is_some_and(|k| !k.trim().is_empty());
        if has_key && include_credentials {
            config.api_key = prov.api_key.clone();
        }
        profiles.push(ProfilePlan {
            conflict: existing_profiles.contains(name),
            name: name.clone(),
            config,
            has_credential: has_key,
            credential_env_var: has_key.then(|| format!("models.providers.{name}.apiKey")),
            credential_file: has_key.then(|| rel_cfg.clone()),
            mcp_refs: Vec::new(),
            source_path: rel_cfg.clone(),
        });
    }

    profiles.sort_by(|a, b| a.name.cmp(&b.name));

    // --- deferred inventory: counted, never dropped ---
    let mut deferred_other: BTreeMap<String, usize> = BTreeMap::new();
    for d in DEFERRED_DIRS {
        let n = count_subdirs(&home.join(d));
        if n > 0 {
            deferred_other.insert((*d).to_string(), n);
        }
    }
    let creds = count_files(&home.join("credentials"));
    if creds > 0 {
        deferred_other.insert("credential_files".into(), creds);
    }
    let backups = count_config_revisions(home);
    if backups > 0 {
        // Named so a reader can see they were found and deliberately excluded,
        // rather than wonder whether the walk missed them.
        deferred_other.insert("config_revisions_excluded".into(), backups);
    }

    mcp_conflicts.sort();
    mcp_conflicts.dedup();
    warnings.sort();
    warnings.dedup();

    Ok(MigrationPlan {
        source: "openclaw",
        source_home: home.to_path_buf(),
        profiles,
        mcp_servers,
        mcp_conflicts,
        deferred: Deferred::default(),
        deferred_other,
        warnings,
    })
}

/// Split a `provider/model` id. `flux/flux-auto` ⇒ (`flux`, `flux-auto`).
fn split_qualified(v: Option<&str>) -> (Option<String>, Option<String>) {
    match v {
        None => (None, None),
        Some(s) => match s.split_once('/') {
            Some((p, m)) => (Some(p.to_string()), Some(m.to_string())),
            None => (None, Some(s.to_string())),
        },
    }
}

/// The first root-level credential SITE, by name. Deterministic: the candidate
/// order is fixed, not derived from map iteration.
fn first_root_credential(cfg: &OpenClawConfig) -> Option<String> {
    if cfg
        .gateway
        .auth
        .as_ref()
        .and_then(|a| a.token.as_deref())
        .is_some_and(|t| !t.is_empty())
    {
        return Some("gateway.auth.token".to_string());
    }
    if cfg
        .gateway
        .remote
        .as_ref()
        .and_then(|a| a.token.as_deref())
        .is_some_and(|t| !t.is_empty())
    {
        return Some("gateway.remote.token".to_string());
    }
    let mut names: Vec<&String> = cfg.channels.keys().collect();
    names.sort();
    for name in names {
        if cfg.channels[name]
            .bot_token
            .as_deref()
            .is_some_and(|t| !t.is_empty())
        {
            return Some(format!("channels.{name}.botToken"));
        }
    }
    None
}

fn map_mcp(s: &OpenClawMcpServer) -> McpServerConfig {
    let transport = match s.transport.as_deref() {
        Some("sse") => TransportType::Sse,
        Some("http" | "streamable-http" | "streamable_http") => TransportType::StreamableHttp,
        Some("stdio") => TransportType::Stdio,
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

fn count_subdirs(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0)
}

fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .count()
        })
        .unwrap_or(0)
}

/// Count the backup / last-known-good siblings of the primary config, so the
/// plan can NAME them as deliberately excluded.
fn count_config_revisions(home: &Path) -> usize {
    std::fs::read_dir(home)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    n.starts_with(CONFIG_FILENAME) && n != CONFIG_FILENAME
                })
                .count()
        })
        .unwrap_or(0)
}

// --- OpenClaw source schema (permissive; unknown keys ignored) ---

#[derive(Debug, Deserialize, Default)]
struct OpenClawConfig {
    #[serde(default)]
    agents: OpenClawAgents,
    #[serde(default)]
    models: OpenClawModels,
    #[serde(default)]
    mcp: OpenClawMcp,
    #[serde(default)]
    gateway: OpenClawGateway,
    #[serde(default)]
    channels: BTreeMap<String, OpenClawChannel>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawAgents {
    #[serde(default)]
    defaults: Option<OpenClawDefaults>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct OpenClawDefaults {
    #[serde(default)]
    model: Option<OpenClawModelRef>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct OpenClawModelRef {
    #[serde(default)]
    primary: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawModels {
    #[serde(default)]
    providers: BTreeMap<String, OpenClawProvider>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawProvider {
    #[serde(default, rename = "baseUrl")]
    base_url: Option<String>,
    #[serde(default, rename = "apiKey")]
    api_key: Option<String>,
    #[serde(default)]
    models: Vec<OpenClawModelEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawModelEntry {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawMcp {
    #[serde(default)]
    servers: BTreeMap<String, OpenClawMcpServer>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawMcpServer {
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<std::collections::HashMap<String, String>>,
    url: Option<String>,
    headers: Option<std::collections::HashMap<String, String>>,
    transport: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawGateway {
    #[serde(default)]
    auth: Option<OpenClawToken>,
    #[serde(default)]
    remote: Option<OpenClawToken>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawToken {
    #[serde(default)]
    token: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenClawChannel {
    #[serde(default, rename = "botToken")]
    bot_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_qualified_handles_both_shapes() {
        assert_eq!(
            split_qualified(Some("flux/flux-auto")),
            (Some("flux".into()), Some("flux-auto".into()))
        );
        // Unqualified: the model stands alone rather than being invented into
        // a provider.
        assert_eq!(split_qualified(Some("gpt-5")), (None, Some("gpt-5".into())));
        assert_eq!(split_qualified(None), (None, None));
    }

    #[test]
    fn unknown_keys_are_ignored_permissively() {
        let json = r#"{"agents":{"defaults":{"model":{"primary":"flux/x"},"unknown":1}},
                       "somethingNew":{"a":1}}"#;
        let cfg: OpenClawConfig = serde_json::from_str(json).unwrap();
        let p = cfg.agents.defaults.unwrap().model.unwrap().primary;
        assert_eq!(p.as_deref(), Some("flux/x"));
    }

    #[test]
    fn root_credential_choice_is_deterministic_and_prefers_gateway_auth() {
        let json = r#"{"gateway":{"auth":{"token":"aaaaaaaaaaaa"},
                                  "remote":{"token":"bbbbbbbbbbbb"}},
                       "channels":{"telegram":{"botToken":"cccccccccccc"}}}"#;
        let cfg: OpenClawConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            first_root_credential(&cfg).as_deref(),
            Some("gateway.auth.token")
        );
    }

    #[test]
    fn empty_token_is_not_a_credential() {
        // Negative control: the real install has an empty provider apiKey, so a
        // truthiness bug here would invent a credential that does not exist.
        let json = r#"{"gateway":{"auth":{"token":""}},
                       "channels":{"telegram":{"botToken":"dddddddddddd"}}}"#;
        let cfg: OpenClawConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            first_root_credential(&cfg).as_deref(),
            Some("channels.telegram.botToken")
        );
    }

    #[test]
    fn backup_and_last_good_siblings_are_never_config_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::write(h.join("openclaw.json.bak"), "{}").unwrap();
        std::fs::write(h.join("openclaw.json.last-good"), "{}").unwrap();
        std::fs::write(h.join("openclaw.json.bak.2"), "{}").unwrap();
        // No primary and no legacy name ⇒ there is NO config here, even though
        // three files whose names begin with the config name are present.
        assert!(
            config_path(h).is_none(),
            "a backup sibling was accepted as the config document"
        );

        // With the primary present it wins, and the siblings are counted as
        // excluded revisions rather than becoming sources.
        std::fs::write(h.join("openclaw.json"), "{}").unwrap();
        assert_eq!(
            config_path(h).unwrap().file_name().unwrap(),
            "openclaw.json"
        );
        assert_eq!(count_config_revisions(h), 3);
    }

    #[test]
    fn legacy_config_name_is_honoured_when_primary_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::write(h.join("clawdbot.json"), "{}").unwrap();
        assert_eq!(
            config_path(h).unwrap().file_name().unwrap(),
            "clawdbot.json"
        );
        // And the primary wins the moment it appears.
        std::fs::write(h.join("openclaw.json"), "{}").unwrap();
        assert_eq!(
            config_path(h).unwrap().file_name().unwrap(),
            "openclaw.json"
        );
    }

    #[test]
    fn malformed_config_is_a_named_error_not_an_empty_plan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("openclaw.json"), "{not json").unwrap();
        let err = build_plan(dir.path(), false).unwrap_err().to_string();
        assert!(
            err.contains("not valid OpenClaw JSON"),
            "expected a named parse error, got: {err}"
        );
    }
}
