//! `ScopedToolRegistry` — plugin-facing tool registration.
//!
//! Prefixes every name with the plugin's `tool_namespace`; rejects names that
//! already contain `::` (no double-prefix, no namespace spoofing).

use std::collections::HashMap;

use crate::access_gate::PluginAccessGate;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;
use crate::tool::PluginTool;

/// Host-side trait the wcore-agent adapter implements. Receives the
/// fully-qualified tool name (already namespace-prefixed by
/// `ScopedToolRegistry`) together with the plugin-api-native
/// [`PluginTool`] — data + execution closure. Nothing in this surface
/// names `wcore-tools`; the host adapter reifies the `PluginTool`.
pub trait ToolRegistrar: Send {
    /// Register a fully-qualified tool name (already prefixed with the
    /// plugin's `tool_namespace`) together with the `PluginTool`.
    /// Returns `Err` on duplicate from the host's perspective.
    fn host_register(
        &mut self,
        fully_qualified_name: String,
        tool: PluginTool,
    ) -> Result<(), String>;
}

/// Plugin-facing tool registration.
pub struct ScopedToolRegistry<'a> {
    plugin_name: String,
    namespace: String,
    host: &'a mut dyn ToolRegistrar,
    registered: Vec<String>,
}

impl<'a> ScopedToolRegistry<'a> {
    pub fn new(manifest: &PluginManifest, host: &'a mut dyn ToolRegistrar) -> PluginResult<Self> {
        PluginAccessGate::require_tools(manifest)?;
        // SAFETY: `require_tools` returns `Err` (and we `?` out of
        // this function) if `tool_namespace` is None, so reaching
        // this line guarantees the field is Some.
        let namespace = manifest
            .permissions
            .tool_namespace
            .clone()
            .expect("tool_namespace required by access gate");
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            namespace,
            host,
            registered: Vec::new(),
        })
    }

    /// Register a plugin tool. The bare name is read from `tool.name`;
    /// there is no separate `name` argument. Keeps every prior check:
    /// `::`-spoof reject, fully-qualified-name dup check, FQ computation.
    pub fn register_tool(&mut self, tool: PluginTool) -> PluginResult<()> {
        if tool.name.contains("::") {
            return Err(PluginError::ToolNameOutsideNamespace {
                plugin: self.plugin_name.clone(),
                namespace: self.namespace.clone(),
                name: tool.name.clone(),
            });
        }
        let fq = format!("{}::{}", self.namespace, tool.name);
        if self.registered.contains(&fq) {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "tool",
                name: fq,
            });
        }
        self.host.host_register(fq.clone(), tool).map_err(|e| {
            PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "tool",
                name: format!("{fq} ({e})"),
            }
        })?;
        self.registered.push(fq);
        Ok(())
    }
}

/// Cross-plugin namespace claim tracker. Used by the host loader to reject the
/// second plugin that tries to claim the same `tool_namespace`.
#[derive(Debug, Default)]
pub struct NamespaceLedger {
    claimed: HashMap<String, String>, // namespace -> plugin_name
}

impl NamespaceLedger {
    pub fn claim(&mut self, namespace: &str, plugin: &str) -> PluginResult<()> {
        if let Some(existing) = self.claimed.get(namespace)
            && existing != plugin
        {
            return Err(PluginError::NamespaceCollision {
                namespace: namespace.to_string(),
                first: existing.clone(),
                second: plugin.to_string(),
            });
        }
        self.claimed
            .insert(namespace.to_string(), plugin.to_string());
        Ok(())
    }
}
