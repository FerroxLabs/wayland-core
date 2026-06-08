//! `ScopedAgentRegistry` — plugin-registered agent manifests. The host stores
//! them in a per-session agent registry that F2 consumes in W7.

use crate::access_gate::PluginAccessGate;
use crate::agent_manifest::AgentManifest;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;

pub trait AgentRegistrar: Send {
    fn host_register_agent(&mut self, agent: AgentManifest) -> Result<(), String>;
}

pub struct ScopedAgentRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn AgentRegistrar,
    registered: Vec<String>,
}

impl<'a> ScopedAgentRegistry<'a> {
    pub fn new(manifest: &PluginManifest, host: &'a mut dyn AgentRegistrar) -> PluginResult<Self> {
        PluginAccessGate::require_agents(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: Vec::new(),
        })
    }

    pub fn register_agent(&mut self, agent: AgentManifest) -> PluginResult<()> {
        if self.registered.contains(&agent.name) {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "agent",
                name: agent.name,
            });
        }
        let name = agent.name.clone();
        self.host
            .host_register_agent(agent)
            .map_err(|e| PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "agent",
                name: format!("{name} ({e})"),
            })?;
        self.registered.push(name);
        Ok(())
    }
}
