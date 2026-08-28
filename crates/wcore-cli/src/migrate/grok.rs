//! grok → wayland-core source loader + mappers (26-SC2 peer coverage).
//!
//! The third peer, alongside [`super::hermes`] and [`super::openclaw`]. Pure
//! reconnaissance — nothing here writes; [`super::apply_plan`] is the only
//! writer. Same idiom as the other two sources: permissive deserialization that
//! ignores unknown keys, a deterministic total order, and per-entry warnings
//! rather than a hard failure that discards the whole plan.
//!
//! # On-disk format
//!
//! GROUNDED in the peer's OWN source under `grok-build`, not inferred from the
//! shape of the other two peers and not inferred from the repository layout
//! (the repo is the product's source tree, not a user install):
//!
//! ```text
//! home            $GROK_HOME, else ~/.grok        xai-grok-config/src/paths.rs:28-47
//! config          <home>/config.toml              xai-grok-config/src/lib.rs:4-6
//! model           [models] default = "<id>"       xai-grok-shell/src/agent/config.rs:975-977
//! mcp             [mcp_servers.<name>]            xai-grok-shell/src/util/config/mcp.rs:419
//! credential      <home>/auth.json                xai-grok-shell/src/auth/flow.rs:1951
//! user skills     <home>/skills/<n>/SKILL.md      xai-grok-shell/src/builtin.rs:75,134,150
//! vendor skills   <home>/bundled, server-skills   xai-grok-shell/src/inspect/mod.rs:1820,1828
//! personas        <home>/personas/*.toml          xai-grok-shell/src/config/mod.rs:385-392
//! memory          <home>/memory                   grok_home().join("memory")
//! ```
//!
//! # The credential is a REFERENCE, never a value
//!
//! `auth.json` holds an OIDC session (`key` + `refresh_token`), not a provider
//! API key — grok's own resolution order is `--api-key` → `$XAI_API_KEY` →
//! `auth.json` (`trace_classifier/mod.rs:1043-1057`). A session token written
//! into wayland's `config.toml` as `api_key` would be a short-lived value in a
//! long-lived field, so it is recorded by NAME only and `--include-credentials`
//! deliberately does not promote it. This is the same call
//! [`super::openclaw::build_plan`] makes for a gateway token.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use wcore_config::config::{McpServerConfig, ProfileConfig, TransportType};
use wcore_config::portability::GROK_ROOT_PROFILE_ID;

use super::hermes::relative_to;
use super::{Deferred, MigrationPlan, ProfilePlan};

/// The primary config document, at the home root.
const CONFIG_FILENAME: &str = "config.toml";
/// The OIDC session store. Evidence of a grok home even with no `config.toml`.
const AUTH_FILENAME: &str = "auth.json";
/// `$GROK_HOME` overrides the default home. Read from the peer's own resolver.
const HOME_ENV: &str = "GROK_HOME";
/// The default home directory name under `$HOME`.
const STATE_DIRNAME: &str = ".grok";

/// wayland-core's builtin provider for grok's models.
///
/// NOT invented: `crates/wcore-config/src/config.rs:2545` parses both `"xai"`
/// and `"grok"` to `ProviderType::Xai`, and `"xai"` is the canonical spelling
/// `provider_name` emits (`config.rs:1773`).
const PROVIDER: &str = "xai";

/// Trees counted for the deferred inventory rather than imported.
///
/// `bundled/` and `server-skills/` are the VENDOR's and the SERVER's shipped
/// catalogs, not the user's setup — the same distinction F26-GRADE-M1 drew for
/// Hermes's `hermes-agent/optional-skills/`. Copying them would inflate an
/// "imported" count without migrating anything the user authored, and they are
/// re-obtained by installing the product. Counted so a reader can see they were
/// found and deliberately excluded, rather than wonder whether the walk missed
/// them.
const DEFERRED_DIRS: &[&str] = &[
    "bundled",
    "hooks",
    "marketplace-cache",
    "plugin-data",
    "server-skills",
    "sessions",
    "vendor",
    "workspace",
    "worktrees",
];

/// Resolve and validate the grok home to import from.
///
/// A home is acceptable when EITHER `config.toml` OR `auth.json` exists.
/// Requiring only `config.toml` would make a logged-in-but-never-configured
/// install unimportable — grok's `grok_home()` creates the directory on every
/// start and `config.toml` appears only once a setting is written, so an
/// auth-only home is the NORMAL state of a fresh install, not an edge case.
/// This is the F26-01 gap-4 lesson applied before it could be re-earned.
pub fn detect_home(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        let home = p.to_path_buf();
        if !is_grok_home(&home) {
            bail!(
                "no grok setup found under {} — expected {} or {}",
                home.display(),
                CONFIG_FILENAME,
                AUTH_FILENAME
            );
        }
        return Ok(home);
    }

    if let Ok(v) = std::env::var(HOME_ENV) {
        let t = v.trim();
        if !t.is_empty() {
            let home = PathBuf::from(t);
            if is_grok_home(&home) {
                return Ok(home);
            }
        }
    }

    let default = dirs::home_dir()
        .context("cannot resolve the home directory to locate ~/.grok")?
        .join(STATE_DIRNAME);
    if is_grok_home(&default) {
        return Ok(default);
    }
    bail!(
        "no grok setup found — looked for {} or {} in {}",
        CONFIG_FILENAME,
        AUTH_FILENAME,
        default.display()
    )
}

/// True when a directory carries either grok document.
fn is_grok_home(home: &Path) -> bool {
    home.join(CONFIG_FILENAME).is_file() || home.join(AUTH_FILENAME).is_file()
}

/// Build the migration plan from a grok home.
///
/// `include_credentials` is accepted for signature parity with the other two
/// sources and deliberately UNUSED: see the module header — grok's only
/// credential is an OIDC session, which is recorded by reference and never
/// promoted into a `ProfileConfig::api_key`.
pub fn build_plan(home: &Path, _include_credentials: bool) -> Result<MigrationPlan> {
    let cfg_path = home.join(CONFIG_FILENAME);
    let mut warnings: Vec<String> = Vec::new();

    // A malformed config is a NAMED error, never a panic and never a silently
    // empty plan that would read as success. An ABSENT config is not an error:
    // an auth-only home is legal (see `detect_home`).
    let cfg: GrokConfig = if cfg_path.is_file() {
        let raw = std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?;
        match toml::from_str(&raw) {
            Ok(c) => c,
            Err(e) => bail!("{} is not valid grok TOML: {e}", cfg_path.display()),
        }
    } else {
        GrokConfig::default()
    };

    let existing_profiles = super::hermes::existing_profile_names();
    let existing_mcp = super::hermes::existing_mcp_names();

    let mut mcp_servers: BTreeMap<String, McpServerConfig> = BTreeMap::new();
    let mut mcp_conflicts: Vec<String> = Vec::new();

    // A DISABLED server is not imported: `enabled = false` is the user's own
    // statement that they do not want it running, and carrying it across as an
    // active server would re-enable something they turned off. Named in the
    // warnings so the omission is visible rather than silent.
    for (name, srv) in &cfg.mcp_servers {
        if srv.enabled == Some(false) {
            warnings.push(format!(
                "mcp server {name:?} is disabled in {CONFIG_FILENAME} — not imported"
            ));
            continue;
        }
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
    let mut refs: Vec<String> = cfg
        .mcp_servers
        .iter()
        .filter(|(_, s)| s.enabled != Some(false))
        .map(|(n, _)| n.clone())
        .collect();
    refs.sort();
    refs.dedup();

    // --- the root setup ---
    //
    // grok has ONE user-global setup, not a set of profiles: its per-agent
    // customisation is `personas/`, which is a subagent instruction body rather
    // than a provider binding. Inventing one wayland profile per persona would
    // publish profiles the peer never had.
    let model = cfg.models.default.clone();
    let mut config = ProfileConfig {
        provider: model.as_ref().map(|_| PROVIDER.to_string()),
        model,
        ..Default::default()
    };
    if !refs.is_empty() {
        config.mcp_servers = Some(refs.clone());
    }

    let auth_path = home.join(AUTH_FILENAME);
    let has_credential = auth_path.is_file();

    // The silently-empty result that reads as success — the same shape the
    // Hermes source names for a config that parses to nothing.
    if config.model.is_none() && !has_credential {
        warnings.push(format!(
            "{CONFIG_FILENAME} declares no [models] default and there is no \
             {AUTH_FILENAME} — the root setup imported empty"
        ));
    }

    let profiles = vec![ProfilePlan {
        conflict: existing_profiles.contains(GROK_ROOT_PROFILE_ID),
        name: GROK_ROOT_PROFILE_ID.to_string(),
        config,
        has_credential,
        // By NAME only. There is deliberately no branch that reads the value.
        credential_env_var: has_credential.then(|| format!("{AUTH_FILENAME}.key")),
        credential_file: has_credential.then(|| relative_to(home, &auth_path)),
        mcp_refs: refs,
        source_path: if cfg_path.is_file() {
            relative_to(home, &cfg_path)
        } else {
            relative_to(home, &auth_path)
        },
    }];

    // --- deferred inventory: counted, never dropped ---
    let mut deferred_other: BTreeMap<String, usize> = BTreeMap::new();
    for d in DEFERRED_DIRS {
        let n = count_subdirs(&home.join(d));
        if n > 0 {
            deferred_other.insert((*d).to_string(), n);
        }
    }

    let deferred = Deferred {
        skills: count_subdirs(&home.join("skills")),
        personas: count_toml_files(&home.join("personas")),
        memory_files: count_memory_notes(&home.join("memory")),
    };

    mcp_conflicts.sort();
    mcp_conflicts.dedup();
    warnings.sort();
    warnings.dedup();

    Ok(MigrationPlan {
        source: "grok",
        source_home: home.to_path_buf(),
        profiles,
        mcp_servers,
        mcp_conflicts,
        deferred,
        deferred_other,
        warnings,
    })
}

fn map_mcp(s: &GrokMcpServer) -> McpServerConfig {
    // Same inference the other two sources use, so one vocabulary describes all
    // three peers rather than three near-identical ones that can drift.
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
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0)
}

/// grok personas are `personas/<name>.toml` — a file each, not a directory.
fn count_toml_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| {
                    e.path().is_file()
                        && e.path().extension().and_then(|x| x.to_str()) == Some("toml")
                })
                .count()
        })
        .unwrap_or(0)
}

/// `*.md` notes, excluding the `MEMORY.md` entrypoint — the same exclusion the
/// Hermes source and `wcore-memory`'s legacy importer make.
fn count_memory_notes(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| {
                    e.path().extension().and_then(|x| x.to_str()) == Some("md")
                        && e.file_name() != "MEMORY.md"
                })
                .count()
        })
        .unwrap_or(0)
}

// --- grok source schema (permissive; unknown keys ignored) ---

#[derive(Debug, Deserialize, Default)]
struct GrokConfig {
    #[serde(default)]
    models: GrokModels,
    #[serde(default)]
    mcp_servers: BTreeMap<String, GrokMcpServer>,
}

#[derive(Debug, Deserialize, Default)]
struct GrokModels {
    #[serde(default)]
    default: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct GrokMcpServer {
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
    transport: Option<String>,
    enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(config: Option<&str>) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        if let Some(c) = config {
            std::fs::write(d.path().join(CONFIG_FILENAME), c).unwrap();
        }
        d
    }

    #[test]
    fn an_auth_only_home_is_importable() {
        // The regression F26-01 gap 4 recorded for Hermes, refused in advance
        // here: a fresh `grok login` produces auth.json and NO config.toml.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(AUTH_FILENAME), "{}").unwrap();
        assert!(is_grok_home(d.path()));
        let plan = build_plan(d.path(), false).unwrap();
        assert_eq!(plan.profiles.len(), 1);
        assert!(plan.profiles[0].has_credential);
        // Known-negative: an EMPTY directory is not a grok home.
        let empty = tempfile::tempdir().unwrap();
        assert!(!is_grok_home(empty.path()));
        assert!(detect_home(Some(empty.path())).is_err());
    }

    #[test]
    fn the_models_default_becomes_provider_xai_plus_the_bare_model() {
        let d = home_with(Some("[models]\ndefault = \"grok-code-fast-1\"\n"));
        let plan = build_plan(d.path(), false).unwrap();
        let root = &plan.profiles[0];
        assert_eq!(root.name, GROK_ROOT_PROFILE_ID);
        assert_eq!(root.config.provider.as_deref(), Some("xai"));
        assert_eq!(root.config.model.as_deref(), Some("grok-code-fast-1"));
        // Known-negative: with NO default, a provider must not be invented —
        // a profile pinned to `xai` with no model is a claim the peer never made.
        let d2 = home_with(Some("[ui]\nscroll_speed = 3\n"));
        let plan2 = build_plan(d2.path(), false).unwrap();
        assert_eq!(plan2.profiles[0].config.provider, None);
        assert_eq!(plan2.profiles[0].config.model, None);
        assert!(
            plan2.warnings.iter().any(|w| w.contains("imported empty")),
            "an empty root must be NAMED, not silently reported as imported: {:?}",
            plan2.warnings
        );
    }

    #[test]
    fn a_disabled_mcp_server_is_not_imported_but_is_named() {
        let d = home_with(Some(
            "[mcp_servers.live]\ncommand = \"npx\"\nargs = [\"srv\"]\n\
             [mcp_servers.off]\ncommand = \"npx\"\nenabled = false\n",
        ));
        let plan = build_plan(d.path(), false).unwrap();
        assert!(plan.mcp_servers.contains_key("live"));
        assert!(
            !plan.mcp_servers.contains_key("off"),
            "a server the user disabled was re-enabled by the import"
        );
        assert!(!plan.profiles[0].mcp_refs.contains(&"off".to_string()));
        assert!(
            plan.warnings.iter().any(|w| w.contains("\"off\"")),
            "the omission was silent: {:?}",
            plan.warnings
        );
    }

    #[test]
    fn transport_is_inferred_the_same_way_as_the_other_peers() {
        let stdio = GrokMcpServer {
            command: Some("srv".into()),
            ..Default::default()
        };
        assert_eq!(map_mcp(&stdio).transport, TransportType::Stdio);
        let http = GrokMcpServer {
            url: Some("https://example.com/mcp".into()),
            ..Default::default()
        };
        assert_eq!(map_mcp(&http).transport, TransportType::StreamableHttp);
        // An explicit declaration outranks the inference.
        let sse = GrokMcpServer {
            url: Some("https://example.com/mcp".into()),
            transport: Some("sse".into()),
            ..Default::default()
        };
        assert_eq!(map_mcp(&sse).transport, TransportType::Sse);
    }

    #[test]
    fn malformed_config_is_a_named_error_not_an_empty_plan() {
        let d = home_with(Some("[models\ndefault = "));
        let err = build_plan(d.path(), false).unwrap_err().to_string();
        assert!(
            err.contains("not valid grok TOML"),
            "expected a named parse error, got: {err}"
        );
    }

    #[test]
    fn the_vendor_catalogs_are_counted_and_never_become_skills() {
        let d = home_with(Some("[models]\ndefault = \"x\"\n"));
        let h = d.path();
        for (root, n) in [
            ("skills", "mine"),
            ("bundled", "help"),
            ("server-skills", "s"),
        ] {
            let sd = h.join(root).join(n);
            std::fs::create_dir_all(&sd).unwrap();
            std::fs::write(sd.join("SKILL.md"), "prose").unwrap();
        }
        let plan = build_plan(h, false).unwrap();
        assert_eq!(
            plan.deferred.skills, 1,
            "only the user root counts as skills"
        );
        assert_eq!(plan.deferred_other.get("bundled"), Some(&1));
        assert_eq!(plan.deferred_other.get("server-skills"), Some(&1));

        // The claim that matters is about the SCAN, not the count: the shared
        // walker must reach the user root and must NOT reach either catalog.
        let found = crate::migrate::quarantine::scan_peer_skills(h);
        let ids: Vec<&str> = found.iter().map(|(f, _)| f.id.as_str()).collect();
        assert!(
            ids.contains(&"skill:skills/mine"),
            "user skill missed: {ids:?}"
        );
        assert!(
            !ids.iter().any(|i| i.contains("bundled")),
            "the vendor catalog was imported: {ids:?}"
        );
        assert!(
            !ids.iter().any(|i| i.contains("server-skills")),
            "the server catalog was imported: {ids:?}"
        );
    }
}
