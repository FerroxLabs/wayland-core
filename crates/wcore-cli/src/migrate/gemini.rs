//! gemini-cli → wayland-core source loader + mappers (26-SC2 peer coverage).
//!
//! The fourth peer, completing the set with [`super::hermes`],
//! [`super::openclaw`] and [`super::grok`]. Pure reconnaissance — nothing here
//! writes; [`super::apply_plan`] is the only writer.
//!
//! # On-disk format
//!
//! GROUNDED in the peer's OWN source under `gemini-cli`. **The `.gemini/`
//! directory checked into that repository is a PROJECT config, not a user
//! home** — reading the importer's format off it would have targeted the wrong
//! tree, which is exactly the mistake `openclaw.rs` avoided by reading
//! `src/config/paths.ts` instead of a directory listing:
//!
//! ```text
//! home        <$GEMINI_CLI_HOME or $HOME>/.gemini   utils/paths.ts:13,22-28 + config/storage.ts:54-60
//! settings    <home>/settings.json                  config/storage.ts:78-80
//! model       model.name                            cli/config/settingsSchema.ts:1062-1079
//! mcp         mcpServers: {}                        cli/config/settingsSchema.ts:161-174
//! transports  command/args/env/cwd/url/httpUrl/     core/config/config.ts:478-514
//!             headers/tcp/type
//! auth type   security.auth.selectedType            cli/config/settingsSchema.ts:1977-1985
//! credential  <home>/oauth_creds.json               core/config/storage.ts:22
//! skills      <home>/skills/                        core/config/storage.ts:101-103
//! commands    <home>/commands/*.toml                core/config/storage.ts:97-99
//! memory      <home>/GEMINI.md                      core/tools/memoryTool.ts:11
//! ```
//!
//! # `httpUrl` is deprecated but still read
//!
//! The peer's own comment (`config.ts:496`) marks `httpUrl` deprecated in
//! favour of `url` + `type`. A migration reads what is ON DISK, not what the
//! peer would write today, so both spellings are consumed. Dropping the legacy
//! one would silently lose every server configured before the rename.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use wcore_config::config::{McpServerConfig, ProfileConfig, TransportType};
use wcore_config::portability::GEMINI_ROOT_PROFILE_ID;

use super::hermes::relative_to;
use super::{Deferred, MigrationPlan, ProfilePlan};

/// The settings document, at the home root.
const SETTINGS_FILENAME: &str = "settings.json";
/// The OAuth credential store written by `gemini` after a browser login.
const OAUTH_FILENAME: &str = "oauth_creds.json";
/// Overrides the HOME the `.gemini` directory hangs off — note it replaces the
/// HOME, not the config directory, so the join still happens.
const HOME_ENV: &str = "GEMINI_CLI_HOME";
/// The config directory name under the resolved home.
const STATE_DIRNAME: &str = ".gemini";

/// wayland-core's builtin provider for gemini's models.
///
/// NOT invented: `crates/wcore-config/src/config.rs:2529` parses `"gemini"`
/// (and the `"google"` alias) to `ProviderType::Gemini`.
const PROVIDER: &str = "gemini";

/// Trees counted for the deferred inventory rather than imported.
const DEFERRED_DIRS: &[&str] = &[
    "agents",
    "acknowledgments",
    "commands",
    "extensions",
    "policies",
    "tmp",
    "whisper_models",
];

/// Files counted rather than imported, because each is a live credential store
/// or a machine identity that must NOT travel to another install.
const DEFERRED_CREDENTIAL_FILES: &[&str] = &[
    OAUTH_FILENAME,
    "google_accounts.json",
    "mcp-oauth-tokens.json",
    "a2a-oauth-tokens.json",
    "installation_id",
];

/// Resolve and validate the gemini home to import from.
///
/// Acceptable when EITHER `settings.json` OR `skills/` exists. Settings alone
/// would refuse a home whose only user content is skills, and a skills-only
/// home is real: `gemini` writes `settings.json` lazily, but
/// `~/.gemini/skills/` is populated the first time a skill is installed.
pub fn detect_home(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        let home = p.to_path_buf();
        if !is_gemini_home(&home) {
            bail!(
                "no gemini setup found under {} — expected {} or a skills/ directory",
                home.display(),
                SETTINGS_FILENAME
            );
        }
        return Ok(home);
    }

    // The peer's `homedir()` substitutes $GEMINI_CLI_HOME for the OS home and
    // THEN joins `.gemini`. Reproduced exactly: treating the env var as the
    // config directory itself would look one level too deep.
    let base = match std::env::var(HOME_ENV) {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => dirs::home_dir().context("cannot resolve the home directory to locate ~/.gemini")?,
    };
    let home = base.join(STATE_DIRNAME);
    if is_gemini_home(&home) {
        return Ok(home);
    }
    bail!(
        "no gemini setup found — looked for {} in {}",
        SETTINGS_FILENAME,
        home.display()
    )
}

fn is_gemini_home(home: &Path) -> bool {
    home.join(SETTINGS_FILENAME).is_file() || home.join("skills").is_dir()
}

/// Build the migration plan from a gemini home.
///
/// `include_credentials` is accepted for signature parity and deliberately
/// UNUSED: `settings.json` carries NO api key field at all (verified against
/// `settingsSchema.ts` — there is no `apiKey` key), and gemini's credential is
/// a browser-obtained OAuth grant in `oauth_creds.json`. An OAuth grant is not
/// a `ProfileConfig::api_key`, so it is counted and named, never promoted.
pub fn build_plan(home: &Path, _include_credentials: bool) -> Result<MigrationPlan> {
    let settings_path = home.join(SETTINGS_FILENAME);
    let mut warnings: Vec<String> = Vec::new();

    let cfg: GeminiSettings = if settings_path.is_file() {
        let raw = std::fs::read_to_string(&settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?;
        match serde_json::from_str(&raw) {
            Ok(c) => c,
            Err(e) => bail!("{} is not valid gemini JSON: {e}", settings_path.display()),
        }
    } else {
        GeminiSettings::default()
    };

    let existing_profiles = super::hermes::existing_profile_names();
    let existing_mcp = super::hermes::existing_mcp_names();

    let mut mcp_servers: BTreeMap<String, McpServerConfig> = BTreeMap::new();
    let mut mcp_conflicts: Vec<String> = Vec::new();
    for (name, srv) in &cfg.mcp_servers {
        // A websocket-only server has no wayland transport to land on. Refused
        // by NAME rather than silently mapped to stdio, which would produce a
        // server definition that cannot work and looks like a successful import.
        if srv.tcp.is_some() && srv.command.is_none() && srv.url.is_none() && srv.http_url.is_none()
        {
            warnings.push(format!(
                "mcp server {name:?} is websocket-only (tcp) — wayland-core has no \
                 websocket transport, so it was NOT imported"
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
    let mut refs: Vec<String> = mcp_servers
        .keys()
        .cloned()
        .chain(mcp_conflicts.iter().cloned())
        .collect();
    refs.sort();
    refs.dedup();

    // --- the root setup ---
    //
    // gemini-cli has ONE user-global setup. Its `agents/` tree holds subagent
    // definitions, not provider bindings, so it is counted rather than turned
    // into wayland profiles the peer never had.
    let model = cfg.model.name.clone();
    let mut config = ProfileConfig {
        provider: model.as_ref().map(|_| PROVIDER.to_string()),
        model,
        ..Default::default()
    };
    if !refs.is_empty() {
        config.mcp_servers = Some(refs.clone());
    }

    let oauth_path = home.join(OAUTH_FILENAME);
    let has_credential = oauth_path.is_file();
    let auth_type = cfg
        .security
        .auth
        .selected_type
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);

    if config.model.is_none() && !has_credential {
        warnings.push(format!(
            "{SETTINGS_FILENAME} declares no model.name and there is no \
             {OAUTH_FILENAME} — the root setup imported empty"
        ));
    }
    if let Some(t) = &auth_type {
        warnings.push(format!(
            "gemini authenticates with {t:?}; wayland-core does not consume that \
             grant, so authentication must be re-established here"
        ));
    }

    let profiles = vec![ProfilePlan {
        conflict: existing_profiles.contains(GEMINI_ROOT_PROFILE_ID),
        name: GEMINI_ROOT_PROFILE_ID.to_string(),
        config,
        has_credential,
        // By NAME only. Nothing below reads the file's contents.
        credential_env_var: has_credential.then(|| format!("{OAUTH_FILENAME}.access_token")),
        credential_file: has_credential.then(|| relative_to(home, &oauth_path)),
        mcp_refs: refs,
        source_path: if settings_path.is_file() {
            relative_to(home, &settings_path)
        } else {
            ".".to_string()
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
    let creds = DEFERRED_CREDENTIAL_FILES
        .iter()
        .filter(|f| home.join(f).is_file())
        .count();
    if creds > 0 {
        deferred_other.insert("credential_files".into(), creds);
    }

    let deferred = Deferred {
        skills: count_subdirs(&home.join("skills")),
        // GEMINI.md is gemini's single context/memory document, not a directory
        // of notes — so the count is 0 or 1 by construction.
        personas: 0,
        memory_files: usize::from(home.join(GEMINI_CONTEXT_FILE).is_file()),
    };

    mcp_conflicts.sort();
    mcp_conflicts.dedup();
    warnings.sort();
    warnings.dedup();

    Ok(MigrationPlan {
        source: "gemini",
        source_home: home.to_path_buf(),
        profiles,
        mcp_servers,
        mcp_conflicts,
        deferred,
        deferred_other,
        warnings,
    })
}

/// gemini's context/memory document at the home root.
pub(super) const GEMINI_CONTEXT_FILE: &str = "GEMINI.md";

fn map_mcp(s: &GeminiMcpServer) -> McpServerConfig {
    // `type` is authoritative when present; otherwise `httpUrl` is streamable
    // HTTP by definition and a bare `url` follows the same URL-vs-command
    // inference the other three sources use.
    let url = s.url.clone().or_else(|| s.http_url.clone());
    let transport = match s.transport_type.as_deref() {
        Some("sse") => TransportType::Sse,
        Some("http" | "streamable-http" | "streamable_http") => TransportType::StreamableHttp,
        Some("stdio") => TransportType::Stdio,
        _ if s.http_url.is_some() => TransportType::StreamableHttp,
        _ if url.is_some() && s.command.is_none() => TransportType::StreamableHttp,
        _ => TransportType::Stdio,
    };
    McpServerConfig {
        transport,
        command: s.command.clone(),
        args: s.args.clone(),
        env: s.env.clone(),
        url,
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

// --- gemini source schema (permissive; unknown keys ignored) ---

#[derive(Debug, Deserialize, Default)]
struct GeminiSettings {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, GeminiMcpServer>,
    #[serde(default)]
    model: GeminiModel,
    #[serde(default)]
    security: GeminiSecurity,
}

#[derive(Debug, Deserialize, Default)]
struct GeminiModel {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct GeminiSecurity {
    #[serde(default)]
    auth: GeminiAuth,
}

#[derive(Debug, Deserialize, Default)]
struct GeminiAuth {
    #[serde(default, rename = "selectedType")]
    selected_type: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct GeminiMcpServer {
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    url: Option<String>,
    #[serde(rename = "httpUrl")]
    http_url: Option<String>,
    headers: Option<HashMap<String, String>>,
    tcp: Option<String>,
    #[serde(rename = "type")]
    transport_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(settings: Option<&str>) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        if let Some(s) = settings {
            std::fs::write(d.path().join(SETTINGS_FILENAME), s).unwrap();
        }
        d
    }

    #[test]
    fn a_skills_only_home_is_importable() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("skills")).unwrap();
        assert!(is_gemini_home(d.path()));
        let plan = build_plan(d.path(), false).unwrap();
        assert_eq!(plan.profiles.len(), 1);
        // Known-negative: an empty directory is NOT a gemini home.
        let empty = tempfile::tempdir().unwrap();
        assert!(!is_gemini_home(empty.path()));
        assert!(detect_home(Some(empty.path())).is_err());
    }

    #[test]
    fn model_name_becomes_provider_gemini_plus_the_bare_model() {
        let d = home_with(Some(r#"{"model":{"name":"gemini-3-pro"}}"#));
        let plan = build_plan(d.path(), false).unwrap();
        let root = &plan.profiles[0];
        assert_eq!(root.name, GEMINI_ROOT_PROFILE_ID);
        assert_eq!(root.config.provider.as_deref(), Some("gemini"));
        assert_eq!(root.config.model.as_deref(), Some("gemini-3-pro"));
        // Known-negative: the repo's OWN settings.json shape — experimental
        // flags and nothing else — must NOT invent a provider.
        let d2 = home_with(Some(
            r#"{"experimental":{"voiceMode":true},"general":{"devtools":true}}"#,
        ));
        let plan2 = build_plan(d2.path(), false).unwrap();
        assert_eq!(plan2.profiles[0].config.provider, None);
        assert!(plan2.warnings.iter().any(|w| w.contains("imported empty")));
    }

    #[test]
    fn the_deprecated_http_url_spelling_is_still_read() {
        // A migration reads what is on disk. Losing `httpUrl` would silently
        // drop every server configured before the peer's own rename.
        let legacy = GeminiMcpServer {
            http_url: Some("https://example.com/mcp".into()),
            ..Default::default()
        };
        let m = map_mcp(&legacy);
        assert_eq!(m.transport, TransportType::StreamableHttp);
        assert_eq!(m.url.as_deref(), Some("https://example.com/mcp"));

        // `type` outranks the inference; a stdio server stays stdio.
        let sse = GeminiMcpServer {
            url: Some("https://example.com/mcp".into()),
            transport_type: Some("sse".into()),
            ..Default::default()
        };
        assert_eq!(map_mcp(&sse).transport, TransportType::Sse);
        let stdio = GeminiMcpServer {
            command: Some("npx".into()),
            ..Default::default()
        };
        assert_eq!(map_mcp(&stdio).transport, TransportType::Stdio);
    }

    #[test]
    fn a_websocket_only_server_is_refused_by_name_not_mapped_to_stdio() {
        let d = home_with(Some(
            r#"{"mcpServers":{"ws":{"tcp":"ws://localhost:9000"},
                              "ok":{"command":"npx","args":["srv"]}}}"#,
        ));
        let plan = build_plan(d.path(), false).unwrap();
        assert!(plan.mcp_servers.contains_key("ok"));
        assert!(
            !plan.mcp_servers.contains_key("ws"),
            "a websocket server was mapped onto a transport that cannot serve it"
        );
        assert!(
            plan.warnings.iter().any(|w| w.contains("websocket-only")),
            "the refusal was silent: {:?}",
            plan.warnings
        );
    }

    #[test]
    fn malformed_settings_is_a_named_error_not_an_empty_plan() {
        let d = home_with(Some("{not json"));
        let err = build_plan(d.path(), false).unwrap_err().to_string();
        assert!(
            err.contains("not valid gemini JSON"),
            "expected a named parse error, got: {err}"
        );
    }

    #[test]
    fn credential_stores_are_counted_and_their_values_never_read() {
        let d = home_with(Some(
            r#"{"security":{"auth":{"selectedType":"oauth-personal"}}}"#,
        ));
        let h = d.path();
        std::fs::write(h.join(OAUTH_FILENAME), r#"{"access_token":"SECRET-VALUE"}"#).unwrap();
        std::fs::write(h.join("google_accounts.json"), "{}").unwrap();
        let plan = build_plan(h, true).unwrap();

        assert_eq!(plan.deferred_other.get("credential_files"), Some(&2));
        let root = &plan.profiles[0];
        assert!(root.has_credential);
        assert_eq!(
            root.credential_env_var.as_deref(),
            Some("oauth_creds.json.access_token")
        );
        // The assertion that can fail: even with `include_credentials = true`,
        // NO field of the plan holds the token. Checked against the value, not
        // against the field name — a rename would otherwise pass this.
        let rendered = format!("{plan:?}");
        assert!(
            !rendered.contains("SECRET-VALUE"),
            "an OAuth grant reached the plan"
        );
        assert!(root.config.api_key.is_none());
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("oauth-personal") && w.contains("re-established")),
            "the operator was not told the grant does not travel: {:?}",
            plan.warnings
        );
    }
}
