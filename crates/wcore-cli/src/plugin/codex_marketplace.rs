// Codex marketplace catalog parsing (`.agents/plugins/marketplace.json`).
//
// Foreign-format knowledge only. The output is the SAME
// `(MarketplaceMeta, Vec<SourceEntry>)` the Claude Code parser produces, so
// everything past this module — quarantine clone, traversal containment,
// format detection, lowering, install plan, consent, provenance, commit — is
// shared, not duplicated.
//
// Schema source (first-party, fetched 2026-08-27):
//   * openai/codex `codex-rs/core-plugins/src/marketplace.rs`
//     — `RawMarketplaceManifest`, `RawMarketplaceManifestPluginSource`
//       (untagged `Path(String) | Object`), `RawMarketplaceManifestPluginSourceObject`
//       (`#[serde(tag = "source", rename_all = "lowercase")]`, variants
//       `local` / `url` / `git-subdir` / `npm`), `MARKETPLACE_MANIFEST_RELATIVE_PATHS`.
//   * openai/codex `codex-rs/skills/.../references/plugin-json-spec.md`
//     — the published field guide (`policy.installation`,
//       `policy.authentication`, `policy.products`, `category`).
//
// | Codex catalog field | Wayland | Note |
// |---|---|---|
// | `name` | `MarketplaceMeta::name` | |
// | `interface.displayName` | — | catalog display chrome; not a plugin capability |
// | `plugins[].name` | `SourceEntry::name` | |
// | `plugins[].source` = `"./p"` | `SourceKind::RelativePath` | traversal-checked |
// | `plugins[].source` = `{source:"local", path}` | `SourceKind::RelativePath` | traversal-checked |
// | `plugins[].source` = `{source:"url", url, path?, ref?, sha?}` | `Url`, or `GitSubdir` when `path` is present | `path` traversal-checked |
// | `plugins[].source` = `{source:"git-subdir", url, path, ref?, sha?}` | `GitSubdir` | `path` traversal-checked |
// | `plugins[].source` = `{source:"npm", package, version?, registry?}` | `Npm` | acquisition still deferred (needs a Node toolchain) |
// | `plugins[].version` / `description` | `SourceEntry` | entry-level manifest fallback |
// | `plugins[].policy.installation` | LOSSY | Wayland install is always explicit + consented |
// | `plugins[].policy.authentication` | LOSSY | no connector auth timing to honor |
// | `plugins[].policy.products` | LOSSY | no product gating |
// | `plugins[].category` | LOSSY | directory display bucket |
//
// Codex has no `github` shorthand and no `metadata.pluginRoot`, and no
// per-entry `strict`; entries default to `strict: true` to match the Claude
// Code parser's default.

use serde_json::Value;
use wcore_pluginsrc::model::{IgnoredFeature, SourceEntry, SourceKind};

use crate::plugin::error::{PluginCliError, Result};
use crate::plugin::marketplace::{MarketplaceMeta, reject_traversal};

/// Parse a Codex `.agents/plugins/marketplace.json` body.
pub fn parse_codex_marketplace(json: &str) -> Result<(MarketplaceMeta, Vec<SourceEntry>)> {
    let root: Value = serde_json::from_str(json)?;
    let obj = root.as_object().ok_or_else(|| {
        PluginCliError::Quarantine("marketplace.json: top-level is not an object".into())
    })?;

    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| PluginCliError::Quarantine("marketplace.json: missing 'name'".into()))?
        .to_string();

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
        let source = pe.get("source").ok_or_else(|| {
            PluginCliError::Quarantine(format!(
                "marketplace.json: plugin '{pname}' missing 'source'"
            ))
        })?;
        entries.push(SourceEntry {
            name: pname.clone(),
            kind: parse_source(&pname, source)?,
            // Codex has no per-entry `strict`; match the Claude Code default.
            strict: true,
            declared_version: pe
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            description: pe
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            unsupported: unsupported_entry_fields(&pname, pe),
        });
    }

    Ok((
        MarketplaceMeta {
            name,
            // Codex catalogs carry no owner block and no pluginRoot.
            owner_name: None,
            owner_email: None,
            plugin_root: None,
        },
        entries,
    ))
}

/// Map one Codex `source` field to a [`SourceKind`]. Every path-shaped string
/// is traversal-checked here, before it can reach a clone, a join or a copy.
fn parse_source(plugin: &str, source: &Value) -> Result<SourceKind> {
    if let Some(s) = source.as_str() {
        reject_traversal(s)?;
        return Ok(SourceKind::RelativePath(s.into()));
    }

    let obj = source.as_object().ok_or_else(|| {
        PluginCliError::Quarantine(format!(
            "marketplace.json: plugin '{plugin}' source is neither a string nor an object"
        ))
    })?;
    let ty = obj.get("source").and_then(Value::as_str).ok_or_else(|| {
        PluginCliError::Quarantine(format!(
            "marketplace.json: plugin '{plugin}' source object missing 'source' discriminator"
        ))
    })?;

    let get = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_string);
    let require = |k: &str| {
        get(k).ok_or_else(|| {
            PluginCliError::Quarantine(format!(
                "marketplace.json: plugin '{plugin}' '{ty}' source missing '{k}'"
            ))
        })
    };

    match ty {
        "local" => {
            let path = require("path")?;
            reject_traversal(&path)?;
            Ok(SourceKind::RelativePath(path.into()))
        }
        // Codex folds the subdir case into `url` by allowing an optional
        // `path`. Lower it to the same GitSubdir the Claude Code parser
        // produces so the quarantine clone has one subdir code path.
        "url" => {
            let url = require("url")?;
            match get("path") {
                Some(path) => {
                    reject_traversal(&path)?;
                    Ok(SourceKind::GitSubdir {
                        url,
                        path,
                        git_ref: get("ref"),
                        sha: get("sha"),
                    })
                }
                None => Ok(SourceKind::Url {
                    url,
                    git_ref: get("ref"),
                    sha: get("sha"),
                }),
            }
        }
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

/// Catalog-level policy/display fields Wayland does not honor. Reported only
/// when the entry actually declares them, so a minimal catalog produces a clean
/// plan and a policy-bearing one can never look like parity.
fn unsupported_entry_fields(
    plugin: &str,
    pe: &serde_json::Map<String, Value>,
) -> Vec<IgnoredFeature> {
    let mut out = Vec::new();
    if let Some(policy) = pe.get("policy").and_then(Value::as_object) {
        if let Some(v) = policy.get("installation").and_then(Value::as_str) {
            out.push(IgnoredFeature {
                kind: "marketplace-policy".to_string(),
                detail: format!(
                    "{plugin}: policy.installation='{v}' not honored — every Wayland install \
                     is explicit and consented"
                ),
            });
        }
        if let Some(v) = policy.get("authentication").and_then(Value::as_str) {
            out.push(IgnoredFeature {
                kind: "marketplace-policy".to_string(),
                detail: format!("{plugin}: policy.authentication='{v}' not honored"),
            });
        }
        if policy.contains_key("products") {
            out.push(IgnoredFeature {
                kind: "marketplace-policy".to_string(),
                detail: format!("{plugin}: policy.products gating not honored"),
            });
        }
    }
    if let Some(v) = pe.get("category").and_then(Value::as_str) {
        out.push(IgnoredFeature {
            kind: "marketplace-display".to_string(),
            detail: format!("{plugin}: category='{v}' not carried"),
        });
    }
    out
}
