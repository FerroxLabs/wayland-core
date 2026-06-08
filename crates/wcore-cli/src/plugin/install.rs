// M5.4: install / remove / list. Install records are JSON files at
// `<install_root>/<name>.json`. Keeping the on-disk format JSON (not
// TOML) means the lib can use serde_json — already a workspace dep —
// without dragging extra parsing code into the install path.

use super::error::{PluginCliError, Result};
use super::manifest::PluginManifest;
use super::registry::Registry;
use super::resolver::{Resolver, validate_plugin_name};
use std::path::Path;

/// Install a plugin from an in-memory `Registry` (local file or
/// embedded default). Validates the name, copies the manifest, writes a
/// JSON install record. NEVER fetches anything over the network.
pub fn install_from_registry(registry: &Registry, name: &str, install_root: &Path) -> Result<()> {
    validate_plugin_name(name)?;
    let mf = registry.get(name)?.clone();
    write_install_record(&mf, install_root)
}

/// Install a plugin via a resolver. This is the entry point for
/// remote-registry installs (`GitHubReleasesResolver`) as well as the
/// trait-based local path (`LocalFileResolver`).
pub fn install_via_resolver(
    resolver: &dyn Resolver,
    name: &str,
    install_root: &Path,
) -> Result<()> {
    let mf = resolver.resolve_manifest(name)?;
    write_install_record(&mf, install_root)
}

/// Common tail for both install paths: re-validate the manifest's own
/// `name` field (defense in depth — the resolver MIGHT have produced a
/// manifest with a different name than the one we asked for), then
/// write the JSON install record.
fn write_install_record(mf: &PluginManifest, install_root: &Path) -> Result<()> {
    validate_plugin_name(&mf.name)?;
    std::fs::create_dir_all(install_root)?;
    // `Path::join` keeps cross-platform path semantics; the name has
    // already been validated against `^[a-z][a-z0-9-]*$` so the
    // resulting file is always a simple filename, never a traversal.
    let target = install_root.join(format!("{}.json", mf.name));
    if target.exists() {
        return Err(PluginCliError::AlreadyInstalled(mf.name.clone()));
    }
    let body = serde_json::to_string_pretty(mf)?;
    wcore_config::atomic_write(&target, body.as_bytes())?;
    Ok(())
}

/// Remove an installed plugin's record. Returns `NotInstalled` if the
/// record doesn't exist — never silently succeeds.
pub fn remove(install_root: &Path, name: &str) -> Result<()> {
    validate_plugin_name(name)?;
    let target = install_root.join(format!("{name}.json"));
    if !target.exists() {
        return Err(PluginCliError::NotInstalled(name.to_string()));
    }
    std::fs::remove_file(&target)?;
    Ok(())
}

/// List installed plugins as parsed `PluginManifest`s, sorted by name.
/// Returns an empty Vec when `install_root` doesn't exist yet (first
/// run case) rather than erroring out.
pub fn list_installed(install_root: &Path) -> Result<Vec<PluginManifest>> {
    let mut out = Vec::new();
    if !install_root.exists() {
        return Ok(out);
    }
    for ent in std::fs::read_dir(install_root)? {
        let path = ent?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)?;
        let mf: PluginManifest = serde_json::from_str(&raw)?;
        out.push(mf);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
