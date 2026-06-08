//! v0.7.0 Task 3.A.1: built-in agent pack registry.
//!
//! Bundles 13 default agent manifests as `include_str!`-embedded TOML and
//! answers lookups by name. Phase 3.A.2/3.A.3/3.A.4 fill the manifests
//! with role-specific system prompts, tool allowlists, and bench cases;
//! this crate is the registry surface they slot into.

use wcore_plugin_api::AgentManifest;

pub mod factory;
mod manifests;

pub mod bench;

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("invalid manifest TOML for {name}: {source}")]
    Toml {
        name: &'static str,
        #[source]
        source: toml::de::Error,
    },
}

/// Static registry of built-in agent manifests.
///
/// All manifests are parsed from embedded TOML at first call (cached
/// via `OnceLock`). Names are stable identifiers used by `--agent=<name>`.
#[derive(Debug, Clone)]
pub struct AgentPack;

impl AgentPack {
    /// Return every built-in manifest in declaration order.
    pub fn list() -> Vec<AgentManifest> {
        manifests::all().to_vec()
    }

    /// Lookup a built-in manifest by canonical name.
    pub fn get(name: &str) -> Option<AgentManifest> {
        manifests::all().iter().find(|m| m.name == name).cloned()
    }

    /// Names of every built-in manifest (for shell completion + CLI help).
    pub fn names() -> Vec<&'static str> {
        manifests::NAMES.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_thirteen_builtin_agents() {
        let agents = AgentPack::list();
        assert_eq!(
            agents.len(),
            13,
            "expected exactly 13 built-in agents, got {}",
            agents.len()
        );
    }

    #[test]
    fn every_manifest_has_required_fields() {
        for m in AgentPack::list() {
            assert!(!m.name.is_empty(), "agent name must be non-empty");
            assert!(
                !m.description.is_empty(),
                "agent {} missing description",
                m.name
            );
            assert!(
                !m.system_prompt.is_empty(),
                "agent {} missing system_prompt",
                m.name
            );
        }
    }

    #[test]
    fn names_are_unique() {
        let names: Vec<_> = AgentPack::list().into_iter().map(|m| m.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate agent names found");
    }

    #[test]
    fn get_returns_match() {
        let m = AgentPack::get("architect").expect("architect is a built-in");
        assert_eq!(m.name, "architect");
    }

    #[test]
    fn get_returns_none_for_unknown() {
        assert!(AgentPack::get("does-not-exist").is_none());
    }

    #[test]
    fn names_match_list() {
        let from_names: Vec<String> = AgentPack::names().iter().map(|s| s.to_string()).collect();
        let from_list: Vec<String> = AgentPack::list().into_iter().map(|m| m.name).collect();
        assert_eq!(from_names, from_list);
    }
}
