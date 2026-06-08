//! MCP registrar adapter — stores `McpServerSpec` decls. W8 will translate
//! to `wcore_mcp::config::McpServerConfig` and call
//! `wcore_mcp::tool_proxy::register_mcp_tools`.

use wcore_plugin_api::McpServerSpec;
use wcore_plugin_api::registry::mcp::McpRegistrar;

#[derive(Debug, Default)]
pub struct HostMcpRegistrar {
    pub registered: Vec<McpServerSpec>,
}

impl McpRegistrar for HostMcpRegistrar {
    fn host_register_mcp_server(&mut self, server: McpServerSpec) -> Result<(), String> {
        if self.registered.iter().any(|s| s.name == server.name) {
            return Err(format!("duplicate mcp server: {}", server.name));
        }
        self.registered.push(server);
        Ok(())
    }
}
