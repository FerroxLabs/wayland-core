// M5.4: in-memory registry built from a directory of TOML manifests OR
// from the embedded `data/registry-default.json` shipped with the
// binary. Kept dumb on purpose — the CLI dispatcher decides which path
// to construct, then queries `get` / `list_available`.

use super::error::{PluginCliError, Result};
use super::manifest::PluginManifest;
use std::collections::BTreeMap;
use std::path::Path;

pub struct Registry {
    entries: BTreeMap<String, PluginManifest>,
}

impl Registry {
    /// Build a registry by reading every `*.toml` file in `dir`. Files
    /// without a `.toml` extension are skipped silently. Failure to
    /// parse ANY individual manifest fails the whole load — better to
    /// loud-fail than to silently drop a manifest with a typo.
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let mut entries = BTreeMap::new();
        for ent in std::fs::read_dir(dir)? {
            let path = ent?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let raw = std::fs::read_to_string(&path)?;
            let mf: PluginManifest = toml::from_str(&raw)?;
            entries.insert(mf.name.clone(), mf);
        }
        Ok(Self { entries })
    }

    /// Load the embedded default registry. The JSON is compiled into
    /// the binary at build time via `include_str!`, so this method
    /// never touches the filesystem.
    pub fn load_default() -> Result<Self> {
        let raw = include_str!("../../data/registry-default.json");
        let entries: Vec<PluginManifest> = serde_json::from_str(raw)?;
        let mut map = BTreeMap::new();
        for mf in entries {
            map.insert(mf.name.clone(), mf);
        }
        Ok(Self { entries: map })
    }

    /// Return all manifests sorted by name (BTreeMap iteration order).
    pub fn list_available(&self) -> Vec<&PluginManifest> {
        self.entries.values().collect()
    }

    /// Lookup by name. Returns `NotInRegistry` (NOT `InvalidName`) when
    /// the name parses but doesn't exist — the dispatcher already ran
    /// `validate_plugin_name` upstream where appropriate.
    pub fn get(&self, name: &str) -> Result<&PluginManifest> {
        self.entries
            .get(name)
            .ok_or_else(|| PluginCliError::NotInRegistry(name.to_string()))
    }
}
