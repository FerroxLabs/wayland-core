//! `ScopedMcpRegistry` — plugin-registered MCP servers. Host adapter
//! delegates to `wcore_mcp::tool_proxy::register_mcp_tools` after
//! initialize() returns.

use crate::access_gate::PluginAccessGate;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;
use crate::mcp_server_spec::McpServerSpec;

pub trait McpRegistrar: Send {
    fn host_register_mcp_server(&mut self, server: McpServerSpec) -> Result<(), String>;
}

pub struct ScopedMcpRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn McpRegistrar,
    registered: Vec<String>,
}

impl<'a> ScopedMcpRegistry<'a> {
    pub fn new(manifest: &PluginManifest, host: &'a mut dyn McpRegistrar) -> PluginResult<Self> {
        PluginAccessGate::require_mcp_server(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: Vec::new(),
        })
    }

    pub fn register_mcp_server(&mut self, server: McpServerSpec) -> PluginResult<()> {
        if self.registered.contains(&server.name) {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "mcp_server",
                name: server.name,
            });
        }
        let name = server.name.clone();
        self.host.host_register_mcp_server(server).map_err(|e| {
            PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "mcp_server",
                name: format!("{name} ({e})"),
            }
        })?;
        self.registered.push(name);
        Ok(())
    }
}
