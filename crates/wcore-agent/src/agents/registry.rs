//! W7 F2: AgentRegistry — loads `AgentManifest`s from filesystem
//! (`~/.wayland-core/agents/*.yaml` and `<project>/.wayland-core/agents/*.yaml`)
//! and from the W2.5 plugin surface (`ScopedAgentRegistry` via the
//! `AgentRegistrar` impl below). Best-effort: malformed YAML is
//! logged-and-skipped rather than panicking.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Wave RB STABILITY — replaced `std::sync::Mutex` with
// `parking_lot::Mutex` so a panic while holding the agent registry
// lock does not poison it. The four critical sections in this file
// are all short HashMap mutations that cannot leave the registry in
// an inconsistent state on unwind.
use parking_lot::Mutex;

use wcore_plugin_api::agent_manifest::AgentManifest;
use wcore_plugin_api::registry::agents::AgentRegistrar;

/// Source of an agent definition for diagnostics ("user yaml vs plugin").
#[derive(Debug, Clone)]
pub enum AgentSource {
    GlobalYaml(PathBuf),
    ProjectYaml(PathBuf),
    Plugin(String),
}

#[derive(Clone)]
pub struct AgentRegistry {
    inner: Arc<Mutex<HashMap<String, (AgentManifest, AgentSource)>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Load all `.yaml` files under `dir` if it exists. Errors on a
    /// malformed file are logged-and-skipped; the registry is best-effort.
    pub fn load_dir(&self, dir: &Path, source: impl Fn(&Path) -> AgentSource) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let txt = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            match serde_yaml::from_str::<AgentManifest>(&txt) {
                Ok(manifest) => {
                    let mut guard = self.inner.lock();
                    let src = source(&path);
                    guard.insert(manifest.name.clone(), (manifest, src));
                }
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "skipping malformed agent yaml"
                    );
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<AgentManifest> {
        self.inner.lock().get(name).map(|(m, _)| m.clone())
    }

    pub fn list(&self) -> Vec<(String, AgentSource)> {
        self.inner
            .lock()
            .iter()
            .map(|(n, (_, s))| (n.clone(), s.clone()))
            .collect()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Adapter so plugins can register manifests via the W2.5 surface.
impl AgentRegistrar for AgentRegistry {
    fn host_register_agent(&mut self, agent: AgentManifest) -> Result<(), String> {
        let mut guard = self.inner.lock();
        if guard.contains_key(&agent.name) {
            return Err(format!("agent {} already registered", agent.name));
        }
        let name = agent.name.clone();
        guard.insert(name, (agent, AgentSource::Plugin("plugin".into())));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn registry_loads_yaml_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reviewer.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "name: reviewer\ndescription: code reviewer\nsystem_prompt: you review code\n"
        )
        .unwrap();
        let reg = AgentRegistry::new();
        reg.load_dir(dir.path(), |p| AgentSource::GlobalYaml(p.to_path_buf()));
        let got = reg.get("reviewer").expect("must load");
        assert_eq!(got.name, "reviewer");
        assert_eq!(got.description, "code reviewer");
    }

    #[test]
    fn registry_skips_malformed_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("broken.yaml"), "not: valid: yaml: at: all:").unwrap();
        let reg = AgentRegistry::new();
        reg.load_dir(dir.path(), |p| AgentSource::GlobalYaml(p.to_path_buf()));
        assert!(reg.list().is_empty());
    }
}
