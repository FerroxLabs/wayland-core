//! Filesystem refresh is applied only at the engine's admitted user-turn boundary.
use super::*;
use std::sync::atomic::Ordering;

pub(super) struct LocalReload {
    cwd: PathBuf,
    dirs: Vec<PathBuf>,
    plugin_roots: Vec<PathBuf>,
}

impl SkillCatalog {
    pub fn with_local_reload(
        mut self,
        cwd: PathBuf,
        dirs: Vec<PathBuf>,
        plugin_roots: Vec<PathBuf>,
    ) -> Self {
        self.reload = Some(LocalReload {
            cwd,
            dirs,
            plugin_roots,
        });
        self
    }

    /// A synchronous inventory snapshot remains stable during a turn. The
    /// engine calls this between runs, never from a watcher callback during
    /// tool execution. Re-scan also discovers directories that did not exist
    /// when the session began and governance changes outside skill directories.
    pub async fn refresh_local(&self) {
        let Some(config) = &self.reload else {
            return;
        };
        let empty_bundled = crate::bundled::BundledSkillCatalog::new();
        let mut incoming = crate::loader::load_catalog_with_bundled(
            &config.cwd,
            &config.dirs,
            false,
            None,
            &empty_bundled,
        )
        .await;
        for root in &config.plugin_roots {
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            paths.sort();
            for plugin in paths {
                if !plugin.join("plugin.toml").is_file() {
                    continue;
                }
                let Some(dirname) = plugin.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let ns = dirname
                    .split_once('@')
                    .map(|(p, m)| format!("{m}/{p}"))
                    .unwrap_or_else(|| dirname.to_owned());
                incoming.extend(
                    crate::loader::load_plugin_skill_catalog(&plugin.join("skills"), &ns).await,
                );
            }
        }
        let mut cache = self.cache.lock().await;
        let mut refs = self.write_refs();
        let old = refs.clone();
        // Embedded/plugin and MCP refs have their own registration lifecycle.
        let mut updated: Vec<_> = old
            .iter()
            .filter(|r| matches!(r.source, SkillSource::Bundled | SkillSource::Mcp))
            .cloned()
            .collect();
        for item in incoming {
            if !updated.iter().any(|r| r.name == item.name) {
                updated.push(item);
            }
        }
        // Retain prior ranking for surviving names; append new entries in
        // deterministic discovery order. A no-change scan cannot churn a listing.
        updated.sort_by_key(|r| {
            old.iter()
                .position(|o| o.name == r.name)
                .unwrap_or(usize::MAX)
        });
        // Clear bodies even for same-size edits; ref metadata alone is not a
        // content fingerprint. The engine guarantees no active SkillTool here.
        cache.clear();
        if *refs != updated {
            self.inventory_changed.store(true, Ordering::Release);
        }
        *refs = updated;
    }

    /// After inventory edits the transient prompt supersedes the boot listing.
    pub fn inventory_changed(&self) -> bool {
        self.inventory_changed.load(Ordering::Acquire)
    }
}
