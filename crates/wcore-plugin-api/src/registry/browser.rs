//! `ScopedBrowserRegistry` — plugin-facing browser-tool registration.
//!
//! Mirrors the shape of `ScopedSkillRegistry::register_skill` so plugins
//! describe a `BrowserToolSpec` declaratively; the host adapter (in
//! `wcore-agent`, since it can depend on `wcore-browser`) translates the
//! spec into a real `BrowserTool` via `wcore_browser::adapter::from_spec`.
//!
//! Permission gate: requires `register_tools = true` AND a non-empty
//! `tool_namespace` on the manifest. The registry stores at most ONE
//! `BrowserToolSpec` per plugin (a plugin owns one browser surface; if it
//! tries to register twice, we surface `DuplicateRegistration`).

use crate::access_gate::PluginAccessGate;
use crate::browser_spec::BrowserToolSpec;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;

/// Host-side trait the wcore-agent adapter implements. Receives the
/// already-validated spec; host-side translation (BrowserToolSpec →
/// concrete BrowserTool) happens after `initialize()` returns.
pub trait BrowserToolRegistrar: Send {
    fn host_register(&mut self, spec: BrowserToolSpec) -> Result<(), String>;
}

pub struct ScopedBrowserRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn BrowserToolRegistrar,
    registered: bool,
}

impl<'a> ScopedBrowserRegistry<'a> {
    pub fn new(
        manifest: &PluginManifest,
        host: &'a mut dyn BrowserToolRegistrar,
    ) -> PluginResult<Self> {
        // A browser tool is a tool; reuse the standard tools gate so the
        // permission rules stay 1:1 with `ScopedToolRegistry`.
        PluginAccessGate::require_tools(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: false,
        })
    }

    pub fn register_browser_tool(&mut self, spec: BrowserToolSpec) -> PluginResult<()> {
        if self.registered {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "browser_tool",
                name: spec.tool_namespace.clone(),
            });
        }
        self.host
            .host_register(spec.clone())
            .map_err(|e| PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "browser_tool",
                name: format!("{} ({e})", spec.tool_namespace),
            })?;
        self.registered = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_spec::{BrowserPolicySpec, BrowserProviderHint};
    use crate::manifest::{PluginInfo, PluginPermissions};

    struct Capture {
        seen: Vec<BrowserToolSpec>,
    }

    impl BrowserToolRegistrar for Capture {
        fn host_register(&mut self, spec: BrowserToolSpec) -> Result<(), String> {
            self.seen.push(spec);
            Ok(())
        }
    }

    fn manifest_with_tools(namespace: &str) -> PluginManifest {
        PluginManifest {
            plugin: PluginInfo {
                name: "wayland-browser".into(),
                version: "0.1.0".into(),
                description: "test".into(),
                entry: "builtin:wayland_browser".into(),
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

    fn fixture_spec() -> BrowserToolSpec {
        BrowserToolSpec {
            tool_namespace: "Browser".into(),
            preferred_provider: BrowserProviderHint::Auto,
            policy: BrowserPolicySpec::default(),
            allow_cloud: false,
        }
    }

    #[test]
    fn register_once_succeeds_and_captures_spec() {
        let manifest = manifest_with_tools("Browser");
        let mut host = Capture { seen: vec![] };
        {
            let mut reg = ScopedBrowserRegistry::new(&manifest, &mut host).unwrap();
            reg.register_browser_tool(fixture_spec()).unwrap();
        }
        assert_eq!(host.seen.len(), 1);
        assert_eq!(host.seen[0].tool_namespace, "Browser");
    }

    #[test]
    fn second_register_is_rejected() {
        let manifest = manifest_with_tools("Browser");
        let mut host = Capture { seen: vec![] };
        let mut reg = ScopedBrowserRegistry::new(&manifest, &mut host).unwrap();
        reg.register_browser_tool(fixture_spec()).unwrap();
        let r = reg.register_browser_tool(fixture_spec());
        assert!(matches!(r, Err(PluginError::DuplicateRegistration { .. })));
    }
}
