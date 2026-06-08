//! Host-side tool registrar adapter. Captures each plugin-registered
//! `PluginTool` (data + closure) plus its provenance; `runner.rs` turns
//! the captures into `CapturedPluginTool`s and `apply.rs` reifies each
//! into a real `wcore_tools::Tool` via `PluginToolAdapter` (Task 1.7).

use wcore_plugin_api::registry::tools::ToolRegistrar;
use wcore_plugin_api::tool::PluginTool;

/// In-memory collector of plugin-registered tools.
///
/// `#[derive(Debug)]` is intentionally NOT present — `PluginTool` holds
/// `Arc<dyn Fn..>` closures which are not `Debug`.
#[derive(Default)]
pub struct HostToolRegistrar {
    /// `(plugin, fq_name, tool)` — the captured `PluginTool`, still data.
    pub registered: Vec<(String, String, PluginTool)>,
}

impl HostToolRegistrar {
    /// Per-plugin capture sub-view — mirrors `HostCuaRegistrar::capture_for_plugin`.
    /// `runner.rs` mints one per plugin so `CapturedPluginTool.plugin` is accurate.
    pub fn capture_for_plugin(&mut self, plugin: impl Into<String>) -> ToolCaptureFor<'_> {
        ToolCaptureFor {
            plugin: plugin.into(),
            inner: self,
        }
    }
}

/// Borrows `&mut HostToolRegistrar`; stamps the plugin name onto every
/// `host_register` call. `ScopedToolRegistry` is built against this.
pub struct ToolCaptureFor<'a> {
    plugin: String,
    inner: &'a mut HostToolRegistrar,
}

impl<'a> ToolRegistrar for ToolCaptureFor<'a> {
    fn host_register(&mut self, fq: String, tool: PluginTool) -> Result<(), String> {
        if self.inner.registered.iter().any(|(_, f, _)| f == &fq) {
            return Err(format!("duplicate tool: {fq}"));
        }
        self.inner.registered.push((self.plugin.clone(), fq, tool));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_protocol::events::ToolCategory;

    fn tool(name: &str) -> PluginTool {
        PluginTool::host_delegated(name, "d", ToolCategory::Info)
    }

    #[test]
    fn capture_stamps_plugin_name() {
        let mut reg = HostToolRegistrar::default();
        {
            let mut cap = reg.capture_for_plugin("wayland-ijfw");
            cap.host_register("ijfw::run".into(), tool("run")).unwrap();
        }
        assert_eq!(reg.registered.len(), 1);
        assert_eq!(reg.registered[0].0, "wayland-ijfw");
        assert_eq!(reg.registered[0].1, "ijfw::run");
        assert_eq!(reg.registered[0].2.name, "run");
    }

    #[test]
    fn duplicate_fq_name_rejected() {
        let mut reg = HostToolRegistrar::default();
        {
            let mut cap = reg.capture_for_plugin("p");
            cap.host_register("p::a".into(), tool("a")).unwrap();
            let err = cap.host_register("p::a".into(), tool("a")).unwrap_err();
            assert!(err.contains("duplicate"));
        }
        assert_eq!(reg.registered.len(), 1);
    }
}
