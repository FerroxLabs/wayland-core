//! Agent registrar adapter — stores plugin-registered `AgentManifest`s for F2
//! to consume in W7.

use wcore_plugin_api::AgentManifest;
use wcore_plugin_api::registry::agents::AgentRegistrar;

#[derive(Debug, Default)]
pub struct HostAgentRegistrar {
    pub registered: Vec<AgentManifest>,
}

impl AgentRegistrar for HostAgentRegistrar {
    fn host_register_agent(&mut self, agent: AgentManifest) -> Result<(), String> {
        if self.registered.iter().any(|a| a.name == agent.name) {
            return Err(format!("duplicate agent: {}", agent.name));
        }
        self.registered.push(agent);
        Ok(())
    }
}
