// Lane F2: the browse catalog cache.
//
// To list the plugins inside a marketplace, the browse UI needs that
// marketplace's `marketplace.json` — which only exists after a git clone.
// Cloning every registered marketplace each time the `/plugins` overlay opens
// would be unusable, so we persist a lightweight catalog at `marketplace add`
// (and `refresh`) time and read it back instantly. Mirrors Claude Code's
// `plugin-catalog-cache.json`. The cache is advisory display data only — every
// real install still re-resolves the source through the quarantine pipeline, so
// a stale cache can never cause a wrong install, only a stale listing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::plugin::error::Result;

/// One browsable plugin in a marketplace's cached catalog. Cheap display
/// metadata only — the install source is re-resolved live at install time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    plugins: Vec<CatalogEntry>,
}

/// Sanitize a marketplace name into a catalog filename stem (the name is the
/// catalog's declared `name`, already constrained, but be defensive).
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn catalog_path(plugins_root: &Path, marketplace: &str) -> PathBuf {
    plugins_root.join(format!("{}.catalog.json", sanitize(marketplace)))
}

/// Persist a marketplace's plugin catalog. Called at add/refresh time, after
/// the marketplace's `marketplace.json` has been parsed.
pub fn save_catalog(
    plugins_root: &Path,
    marketplace: &str,
    plugins: Vec<CatalogEntry>,
) -> Result<()> {
    std::fs::create_dir_all(plugins_root)?;
    let file = CatalogFile { plugins };
    let bytes = serde_json::to_vec_pretty(&file)?;
    wcore_config::atomic_write(catalog_path(plugins_root, marketplace), &bytes)?;
    Ok(())
}

/// Load a marketplace's cached catalog. Returns an empty vec when no cache
/// exists yet (the browse UI then shows the marketplace with no plugins until a
/// refresh) rather than erroring.
pub fn load_catalog(plugins_root: &Path, marketplace: &str) -> Vec<CatalogEntry> {
    let p = catalog_path(plugins_root, marketplace);
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return Vec::new();
    };
    serde_json::from_str::<CatalogFile>(&raw)
        .map(|f| f.plugins)
        .unwrap_or_default()
}

/// Remove a marketplace's cached catalog (called when the marketplace is
/// removed). Missing file is not an error.
pub fn remove_catalog(plugins_root: &Path, marketplace: &str) -> Result<()> {
    let p = catalog_path(plugins_root, marketplace);
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, desc: &str) -> CatalogEntry {
        CatalogEntry {
            name: name.into(),
            version: Some("1.0.0".into()),
            description: Some(desc.into()),
        }
    }

    #[test]
    fn round_trips_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let plugins = vec![entry("stripe", "payments"), entry("airtable", "tables")];
        save_catalog(root, "official", plugins.clone()).unwrap();
        assert_eq!(load_catalog(root, "official"), plugins);
    }

    #[test]
    fn missing_catalog_loads_empty_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_catalog(tmp.path(), "nope").is_empty());
    }

    #[test]
    fn catalogs_are_namespaced_per_marketplace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        save_catalog(root, "a", vec![entry("one", "x")]).unwrap();
        save_catalog(root, "b", vec![entry("two", "y")]).unwrap();
        assert_eq!(load_catalog(root, "a")[0].name, "one");
        assert_eq!(load_catalog(root, "b")[0].name, "two");
    }

    #[test]
    fn remove_deletes_and_tolerates_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        save_catalog(root, "m", vec![entry("p", "d")]).unwrap();
        assert!(!load_catalog(root, "m").is_empty());
        remove_catalog(root, "m").unwrap();
        assert!(load_catalog(root, "m").is_empty());
        remove_catalog(root, "m").unwrap(); // missing → ok
    }
}
