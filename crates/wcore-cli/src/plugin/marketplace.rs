// Lane C: marketplace catalog parsing + the resolve→clone→lower→plan→commit
// install pipeline.
//
// Foreign-format knowledge is confined to the catalog parsers — the Claude Code
// `marketplace.json` schema in `parse_marketplace`, the Codex
// `.agents/plugins/marketplace.json` schema in `codex_marketplace`. Which
// parser runs is decided ONCE, by `locate_catalog`, from which manifest file the
// source actually ships. Everything past `detect_format` is format-blind and
// flows through the `wcore-pluginsrc` adapters. Nothing here spawns a process
// or writes to the plugin store until `commit_install` is called — planning is
// pure (the InstallPlan is the consent surface).

use std::path::{Path, PathBuf};

use serde_json::Value;
use wcore_pluginsrc::{
    CanonicalDraft, CommitMeta, InstallPlan, Provenance, ResolvedVersion, SourceEntry, SourceKind,
    commit_plan, detect_format,
};

use crate::plugin::error::{PluginCliError, Result};
use crate::plugin::{catalog, known, lockfile, quarantine};

/// Top-level metadata from a foreign marketplace catalog. `owner_*` and
/// `plugin_root` are Claude Code concepts; a Codex catalog leaves them `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceMeta {
    pub name: String,
    pub owner_name: Option<String>,
    pub owner_email: Option<String>,
    /// `metadata.pluginRoot` — base dir prepended to relative-path sources.
    pub plugin_root: Option<String>,
}

/// Parse a `marketplace.json` body into its metadata and the normalized source
/// list. `metadata.pluginRoot` is prepended to every relative-path source. Any
/// `..` in a relative path, git-subdir `path`, or `pluginRoot` is rejected with
/// [`PluginCliError::PathTraversal`] before it can reach a clone or copy.
pub fn parse_marketplace(json: &str) -> Result<(MarketplaceMeta, Vec<SourceEntry>)> {
    let root: Value = serde_json::from_str(json)?;
    let obj = root.as_object().ok_or_else(|| {
        PluginCliError::Quarantine("marketplace.json: top-level is not an object".into())
    })?;

    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| PluginCliError::Quarantine("marketplace.json: missing 'name'".into()))?
        .to_string();

    let owner = obj.get("owner").and_then(Value::as_object);
    let owner_name = owner
        .and_then(|o| o.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let owner_email = owner
        .and_then(|o| o.get("email"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let plugin_root = obj
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|m| m.get("pluginRoot"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(pr) = &plugin_root {
        reject_traversal(pr)?;
    }

    let plugins = obj
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            PluginCliError::Quarantine("marketplace.json: missing 'plugins' array".into())
        })?;

    let mut entries = Vec::with_capacity(plugins.len());
    for p in plugins {
        let pe = p.as_object().ok_or_else(|| {
            PluginCliError::Quarantine("marketplace.json: plugin entry is not an object".into())
        })?;
        let pname = pe
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                PluginCliError::Quarantine("marketplace.json: plugin entry missing 'name'".into())
            })?
            .to_string();
        // Claude Code defaults `strict` to true.
        let strict = pe.get("strict").and_then(Value::as_bool).unwrap_or(true);
        let declared_version = pe
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string);
        let description = pe
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);
        let source = pe.get("source").ok_or_else(|| {
            PluginCliError::Quarantine(format!(
                "marketplace.json: plugin '{pname}' missing 'source'"
            ))
        })?;
        let kind = parse_source(source, plugin_root.as_deref())?;
        entries.push(SourceEntry {
            name: pname,
            kind,
            strict,
            declared_version,
            description,
            // A Claude Code catalog declares no install/auth policy, so it
            // carries nothing that would degrade at this layer.
            unsupported: Vec::new(),
        });
    }

    Ok((
        MarketplaceMeta {
            name,
            owner_name,
            owner_email,
            plugin_root,
        },
        entries,
    ))
}

/// Map one `source` field (a bare string = relative path, or an object with a
/// `source` discriminator) to a [`SourceKind`].
fn parse_source(source: &Value, plugin_root: Option<&str>) -> Result<SourceKind> {
    if let Some(s) = source.as_str() {
        reject_traversal(s)?;
        let joined = match plugin_root {
            Some(root) => format!(
                "{}/{}",
                root.trim_end_matches('/'),
                s.trim_start_matches("./")
            ),
            None => s.to_string(),
        };
        return Ok(SourceKind::RelativePath(PathBuf::from(joined)));
    }

    let obj = source.as_object().ok_or_else(|| {
        PluginCliError::Quarantine(
            "marketplace.json: source is neither a string nor an object".into(),
        )
    })?;
    let ty = obj.get("source").and_then(Value::as_str).ok_or_else(|| {
        PluginCliError::Quarantine(
            "marketplace.json: source object missing 'source' discriminator".into(),
        )
    })?;

    let get = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_string);
    let require = |k: &str| {
        get(k).ok_or_else(|| {
            PluginCliError::Quarantine(format!("marketplace.json: '{ty}' source missing '{k}'"))
        })
    };

    match ty {
        "github" => {
            let repo = require("repo")?;
            reject_traversal(&repo)?;
            Ok(SourceKind::Github {
                repo,
                git_ref: get("ref"),
                sha: get("sha"),
            })
        }
        "url" => Ok(SourceKind::Url {
            url: require("url")?,
            git_ref: get("ref"),
            sha: get("sha"),
        }),
        "git-subdir" => {
            let path = require("path")?;
            reject_traversal(&path)?;
            Ok(SourceKind::GitSubdir {
                url: require("url")?,
                path,
                git_ref: get("ref"),
                sha: get("sha"),
            })
        }
        "npm" => Ok(SourceKind::Npm {
            package: require("package")?,
            version: get("version"),
            registry: get("registry"),
        }),
        other => Err(PluginCliError::Quarantine(format!(
            "marketplace.json: unknown source type '{other}'"
        ))),
    }
}

/// Reject any path that is absolute or contains a `..` (parent-dir) component.
/// Shared by both catalog parsers and the quarantine clone. Rejecting
/// absolute/root/prefix components matters because `Path::join` REPLACES its
/// base when the argument is absolute — `clone_dir.join("/etc")` would escape
/// the clone entirely on Unix, and a `C:\…` prefix does the same on Windows.
/// The source string is attacker-controlled (it comes straight from a foreign
/// `marketplace.json`).
///
/// The rule itself lives in `wcore_pluginsrc::path_guard`, which is also what
/// the plugin-manifest adapters call, so there is exactly ONE implementation of
/// it in the workspace. This wrapper only re-labels the error for the CLI's
/// typed surface.
pub(crate) fn reject_traversal(s: &str) -> Result<()> {
    wcore_pluginsrc::path_guard::reject_traversal(s)
        .map_err(|_| PluginCliError::PathTraversal(s.to_string()))
}

/// Which foreign catalog dialect a marketplace ships. Decided by WHICH manifest
/// file is present, not by sniffing the JSON: file location is the vendor's own
/// declaration of its format, and a location-keyed decision cannot be flipped by
/// hostile content inside the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogDialect {
    ClaudeCode,
    Codex,
}

/// The catalog manifests we ingest, in priority order.
const CATALOG_MANIFESTS: &[(&str, CatalogDialect)] = &[
    (
        ".claude-plugin/marketplace.json",
        CatalogDialect::ClaudeCode,
    ),
    (".agents/plugins/marketplace.json", CatalogDialect::Codex),
];

/// Read whichever catalog manifest `root` ships. `what` names the source in the
/// error so a missing catalog says which marketplace it was looking at.
fn read_catalog(root: &Path, what: &str) -> Result<(String, CatalogDialect)> {
    for (rel, dialect) in CATALOG_MANIFESTS {
        let p = root.join(rel);
        if p.is_file() {
            return Ok((std::fs::read_to_string(&p)?, *dialect));
        }
    }
    Err(PluginCliError::Quarantine(format!(
        "{what} has no .claude-plugin/marketplace.json or .agents/plugins/marketplace.json"
    )))
}

/// Parse a catalog body with the parser its dialect selects.
fn parse_catalog(
    json: &str,
    dialect: CatalogDialect,
) -> Result<(MarketplaceMeta, Vec<SourceEntry>)> {
    match dialect {
        CatalogDialect::ClaudeCode => parse_marketplace(json),
        CatalogDialect::Codex => crate::plugin::codex_marketplace::parse_codex_marketplace(json),
    }
}

// ---------------------------------------------------------------------------
// C4 — resolve → clone → lower → plan → commit
// ---------------------------------------------------------------------------

/// The result of planning an install: a pure [`InstallPlan`] (the consent
/// surface) plus everything `commit_install` needs to write the store. Holds
/// the lowered draft so the commit step never re-fetches or re-lowers.
pub struct PlannedInstall {
    pub plan: InstallPlan,
    pub draft: CanonicalDraft,
    pub fetched_root: PathBuf,
    pub resolved_sha: Option<String>,
    pub format: String,
    pub source_desc: String,
    pub marketplace: String,
}

/// A marketplace entry acquired onto disk, before anything has looked at what
/// FORMAT it is. Extracted from `resolve_and_plan` (F25-04) so the native
/// install path can share the identical catalog lookup, traversal check and
/// quarantine clone instead of carrying a second copy of them.
pub struct ResolvedSource {
    pub entry: SourceEntry,
    pub fetched_root: PathBuf,
    pub resolved_sha: Option<String>,
    pub mref: known::MarketplaceRef,
}

/// Acquire `plugin@market` into the quarantine. Writes nothing to the plugin
/// store and spawns no plugin process.
pub fn resolve_source(
    plugins_root: &Path,
    quarantine_root: &Path,
    market: &str,
    plugin: &str,
) -> Result<ResolvedSource> {
    let mref = known::get_marketplace(plugins_root, market)?
        .ok_or_else(|| PluginCliError::MarketplaceNotFound(market.to_string()))?;

    let market_root = acquire_marketplace(&mref, quarantine_root)?;
    let (mjson, dialect) = read_catalog(&market_root, &format!("marketplace '{market}'"))?;
    let (_meta, entries) = parse_catalog(&mjson, dialect)?;
    let entry = entries
        .into_iter()
        .find(|e| e.name == plugin)
        .ok_or_else(|| PluginCliError::NotInRegistry(format!("{plugin}@{market}")))?;

    let (fetched_root, resolved_sha) = match &entry.kind {
        SourceKind::RelativePath(p) => {
            let root = market_root.join(p);
            ensure_within(&market_root, &root)?;
            (root, None)
        }
        other => {
            let qdir = quarantine_root.join(sanitize(&format!("{market}__{plugin}")));
            let cloned = quarantine::quarantine_clone(other, &qdir)?;
            (cloned.path, Some(cloned.resolved_sha))
        }
    };

    Ok(ResolvedSource {
        entry,
        fetched_root,
        resolved_sha,
        mref,
    })
}

/// Resolve `plugin@market` to an [`InstallPlan`]. Writes NOTHING to the plugin
/// store and spawns no plugin process — it only acquires the source into the
/// quarantine and lowers it. The returned plan is what a user approves before
/// `commit_install` mutates disk.
pub fn resolve_and_plan(
    plugins_root: &Path,
    quarantine_root: &Path,
    market: &str,
    plugin: &str,
) -> Result<PlannedInstall> {
    let resolved = resolve_source(plugins_root, quarantine_root, market, plugin)?;
    let ResolvedSource {
        entry,
        fetched_root,
        resolved_sha,
        mref,
    } = resolved;

    let format = detect_format(&fetched_root).ok_or_else(|| {
        PluginCliError::Quarantine(format!(
            "unrecognized plugin format at {}",
            fetched_root.display()
        ))
    })?;
    let adapter = adapter_for(&format)?;
    let draft = adapter.lower(market, &entry, &fetched_root)?;

    let store_path = plugins_root.join(format!("{}@{market}", draft.name));
    let mut plan = InstallPlan::from_draft(&draft, market, store_path);

    // Catalog-level degradation (foreign install/auth policy, product gating,
    // display category) rides the same consent surface as plugin-level
    // degradation. Without this the plan would show a Codex catalog's policy
    // block as if Wayland honored it.
    plan.ignored.extend(entry.unsupported.iter().cloned());

    // Lane E3: trust ≠ capability. Capability grants come from the manifest
    // permissions; trust is about provenance. An unofficial (user-added)
    // marketplace ships unsigned third-party code, so surface that as a
    // non-blocking warning. Official bundled catalogs are exempt.
    if !mref.official {
        plan.warnings.push(wcore_pluginsrc::PlanWarning {
            kind: "unsigned-source".to_string(),
            component: String::new(),
            detail: format!(
                "marketplace '{market}' is an unofficial source — its plugins are \
                 unsigned third-party code that will run inside your agent"
            ),
        });
    }

    Ok(PlannedInstall {
        plan,
        draft,
        fetched_root,
        resolved_sha,
        format,
        source_desc: quarantine::describe_source(&entry.kind),
        marketplace: market.to_string(),
    })
}

/// Commit a previously-planned install: write the self-contained native plugin
/// dir, then append a commit-pinned lockfile record. `installed_at` is supplied
/// by the caller — lib code never reads the wall clock (keeps this resumable
/// and testable).
pub fn commit_install(
    plugins_root: &Path,
    planned: &PlannedInstall,
    installed_at: String,
) -> Result<PathBuf> {
    let meta = CommitMeta {
        marketplace: &planned.marketplace,
        format: &planned.format,
        resolved_sha: planned.resolved_sha.clone(),
    };
    let dir = commit_plan(&planned.draft, &meta, &planned.fetched_root, plugins_root)?;

    lockfile::record_install(
        plugins_root,
        lockfile::InstallRecord {
            plugin: planned.draft.name.clone(),
            marketplace: planned.marketplace.clone(),
            source: planned.source_desc.clone(),
            resolved_sha: planned.resolved_sha.clone(),
            version: version_string(&planned.draft.version),
            grade: format!("{:?}", planned.plan.grade),
            installed_at,
        },
    )?;

    Ok(dir)
}

/// List marketplace-installed plugins under `plugins_root`. A marketplace
/// install is a flat `<plugin>@<marketplace>/` dir carrying a `provenance.json`
/// sidecar — that sidecar is the marker (and the source of truth for the
/// listing), which also distinguishes these from legacy `<name>.json` installs
/// and from the marketplace's own JSON sidecars in the same root. Unreadable or
/// non-provenance dirs are skipped, never fatal.
pub fn list_marketplace_installed(plugins_root: &Path) -> Result<Vec<Provenance>> {
    let mut out = Vec::new();
    if !plugins_root.exists() {
        return Ok(out);
    }
    for ent in std::fs::read_dir(plugins_root)? {
        let path = ent?.path();
        let prov = path.join("provenance.json");
        if !path.is_dir() || !prov.is_file() {
            continue;
        }
        match std::fs::read_to_string(&prov)
            .ok()
            .and_then(|raw| serde_json::from_str::<Provenance>(&raw).ok())
        {
            Some(p) => out.push(p),
            None => tracing::debug!(path = %prov.display(), "skipping unreadable provenance"),
        }
    }
    out.sort_by(|a, b| a.plugin.cmp(&b.plugin));
    Ok(out)
}

/// Uninstall a marketplace-installed plugin: remove its self-contained install
/// dir and its lockfile record. The dir is located by matching its
/// `provenance.json` (plugin + marketplace) rather than recomputing the
/// sanitized name, so it stays correct regardless of how the name was sanitized
/// at commit time. Returns whether an install dir was removed.
pub fn remove_marketplace_plugin(
    plugins_root: &Path,
    plugin: &str,
    marketplace: &str,
) -> Result<bool> {
    let mut removed = false;
    if plugins_root.exists() {
        for ent in std::fs::read_dir(plugins_root)? {
            let path = ent?.path();
            let prov = path.join("provenance.json");
            if !path.is_dir() || !prov.is_file() {
                continue;
            }
            let matches = std::fs::read_to_string(&prov)
                .ok()
                .and_then(|raw| serde_json::from_str::<Provenance>(&raw).ok())
                .is_some_and(|p| p.plugin == plugin && p.marketplace == marketplace);
            if matches {
                std::fs::remove_dir_all(&path)?;
                removed = true;
            }
        }
    }
    // Drop the lockfile record regardless (keeps the lock consistent even if the
    // dir was already gone).
    lockfile::remove_record(plugins_root, plugin, marketplace)?;
    Ok(removed)
}

/// Make a marketplace's catalog available on disk. Returns the directory that
/// contains one of the [`CATALOG_MANIFESTS`].
fn acquire_marketplace(mref: &known::MarketplaceRef, quarantine_root: &Path) -> Result<PathBuf> {
    acquire_source(&mref.source, &mref.name, quarantine_root)
}

/// Acquire an arbitrary marketplace source string into the quarantine. A
/// local-path source is read in place; anything else is treated as a git URL
/// and quarantine-cloned. `label` only names the quarantine subdir.
fn acquire_source(source: &str, label: &str, quarantine_root: &Path) -> Result<PathBuf> {
    let local = Path::new(source);
    if local.is_dir() {
        return Ok(local.to_path_buf());
    }
    let kind = SourceKind::Url {
        url: source.to_string(),
        git_ref: None,
        sha: None,
    };
    let qdir = quarantine_root.join(sanitize(&format!("mkt__{label}")));
    Ok(quarantine::quarantine_clone(&kind, &qdir)?.path)
}

/// Normalize a user-supplied marketplace source: a local dir stays as-is,
/// `owner/repo` becomes a GitHub URL, anything else is a git URL verbatim.
pub fn normalize_source(source: &str) -> String {
    if Path::new(source).is_dir() {
        return source.to_string();
    }
    // `owner/repo` shorthand: no scheme, exactly one `/`, not a path.
    let looks_like_owner_repo = !source.contains("://")
        && !source.contains(':')
        && source.matches('/').count() == 1
        && !source.starts_with('/')
        && !source.starts_with('.');
    if looks_like_owner_repo {
        return format!("https://github.com/{}.git", source.trim_end_matches('/'));
    }
    source.to_string()
}

/// `plugin marketplace add <source>` — acquire the catalog, learn its declared
/// name, and register it in `known_marketplaces.json`. Returns the catalog
/// metadata (its `name` is the handle used by `install <plugin>@<name>`).
pub fn add_marketplace_source(
    plugins_root: &Path,
    quarantine_root: &Path,
    source: &str,
) -> Result<MarketplaceMeta> {
    let normalized = normalize_source(source);
    let market_root = acquire_source(&normalized, "add", quarantine_root)?;
    let (mjson, dialect) = read_catalog(&market_root, "source")?;
    let (meta, entries) = parse_catalog(&mjson, dialect)?;
    known::add_marketplace(
        plugins_root,
        known::MarketplaceRef {
            name: meta.name.clone(),
            source: normalized,
            official: false,
        },
    )?;
    // Cache the plugin catalog so the browse UI can list this marketplace's
    // plugins without re-cloning it. Display metadata only — installs still
    // re-resolve the live source.
    let catalog = entries
        .iter()
        .map(|e| catalog::CatalogEntry {
            name: e.name.clone(),
            version: e.declared_version.clone(),
            description: e.description.clone(),
        })
        .collect();
    catalog::save_catalog(plugins_root, &meta.name, catalog)?;
    Ok(meta)
}

fn adapter_for(format: &str) -> Result<Box<dyn wcore_pluginsrc::PluginFormatAdapter>> {
    match format {
        "claude-code" => Ok(Box::new(wcore_pluginsrc::claude_code::ClaudeCodeAdapter)),
        "codex" => Ok(Box::new(wcore_pluginsrc::codex::CodexAdapter)),
        other => Err(PluginCliError::Quarantine(format!(
            "no install-time adapter for format '{other}'"
        ))),
    }
}

fn version_string(v: &ResolvedVersion) -> String {
    match v {
        ResolvedVersion::Explicit(s) => s.clone(),
        ResolvedVersion::CommitSha(s) => format!("sha:{s}"),
        ResolvedVersion::Unknown => "unknown".to_string(),
    }
}

/// Confirm `candidate` does not escape `root` after symlink resolution.
fn ensure_within(root: &Path, candidate: &Path) -> Result<()> {
    let rc = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let cc = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_path_buf());
    if !cc.starts_with(&rc) {
        return Err(PluginCliError::PathTraversal(
            candidate.display().to_string(),
        ));
    }
    Ok(())
}

/// Sanitize a string for use as a single on-disk directory component.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
