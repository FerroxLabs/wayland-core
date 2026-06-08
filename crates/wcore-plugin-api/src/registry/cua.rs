//! `ScopedCuaRegistry` — plugin-facing CUA-tool registration.
//!
//! Mirrors `ScopedBrowserRegistry` (W8c.1) shape. Plugins describe a
//! `CuaToolSpec` declaratively; the host adapter (in `wcore-cua` itself
//! today; `wcore-agent` once plugin → tool wiring lands) translates the
//! spec into a real `CuaTool`.
//!
//! Permission gate: requires `register_tools = true` AND a non-empty
//! `tool_namespace` on the manifest. At most ONE `CuaToolSpec` per
//! plugin (a plugin owns one CUA surface; a second registration
//! surfaces `DuplicateRegistration`).

use crate::access_gate::PluginAccessGate;
use crate::cua_spec::CuaToolSpec;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;

/// Host-side trait the wcore-agent adapter implements. Receives the
/// already-validated spec; host-side translation (CuaToolSpec →
/// concrete CuaTool) happens after `initialize()` returns.
pub trait CuaToolRegistrar: Send {
    fn host_register(&mut self, spec: CuaToolSpec) -> Result<(), String>;
}

pub struct ScopedCuaRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn CuaToolRegistrar,
    registered: bool,
}

impl<'a> ScopedCuaRegistry<'a> {
    pub fn new(
        manifest: &PluginManifest,
        host: &'a mut dyn CuaToolRegistrar,
    ) -> PluginResult<Self> {
        PluginAccessGate::require_tools(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: false,
        })
    }

    pub fn register_cua_tool(&mut self, spec: CuaToolSpec) -> PluginResult<()> {
        if self.registered {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "cua_tool",
                name: spec.tool_namespace.clone(),
            });
        }
        self.host
            .host_register(spec.clone())
            .map_err(|e| PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "cua_tool",
                name: format!("{} ({e})", spec.tool_namespace),
            })?;
        self.registered = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cua_spec::CuaPolicySpec;
    use crate::manifest::{PluginInfo, PluginPermissions};

    struct Capture {
        seen: Vec<CuaToolSpec>,
    }

    impl CuaToolRegistrar for Capture {
        fn host_register(&mut self, spec: CuaToolSpec) -> Result<(), String> {
            self.seen.push(spec);
            Ok(())
        }
    }

    fn manifest_with_tools(namespace: &str) -> PluginManifest {
        PluginManifest {
            plugin: PluginInfo {
                name: "wayland-cua".into(),
                version: "0.1.0".into(),
                description: "test".into(),
                entry: "builtin:wayland_cua".into(),
                authors: vec![],
                license: "MIT".into(),
                deferred: false,
            },
            permissions: PluginPermissions {
                register_tools: true,
                tool_namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            capabilities: Default::default(),
            plugin_api_version: None,
            runtime: None,
        }
    }

    fn fixture_spec() -> CuaToolSpec {
        CuaToolSpec {
            tool_namespace: "Cua".into(),
            policy: CuaPolicySpec::default(),
            redact_screenshots: false,
        }
    }

    #[test]
    fn register_once_succeeds_and_captures_spec() {
        let manifest = manifest_with_tools("Cua");
        let mut host = Capture { seen: vec![] };
        {
            let mut reg = ScopedCuaRegistry::new(&manifest, &mut host).unwrap();
            reg.register_cua_tool(fixture_spec()).unwrap();
        }
        assert_eq!(host.seen.len(), 1);
        assert_eq!(host.seen[0].tool_namespace, "Cua");
    }

    #[test]
    fn second_register_is_rejected() {
        let manifest = manifest_with_tools("Cua");
        let mut host = Capture { seen: vec![] };
        let mut reg = ScopedCuaRegistry::new(&manifest, &mut host).unwrap();
        reg.register_cua_tool(fixture_spec()).unwrap();
        let r = reg.register_cua_tool(fixture_spec());
        assert!(matches!(r, Err(PluginError::DuplicateRegistration { .. })));
    }
}
