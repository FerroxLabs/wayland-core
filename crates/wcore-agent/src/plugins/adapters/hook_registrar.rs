//! In-memory hook registrar adapter. W2.5 captures (phase, name); Task 1.3
//! adds per-plugin provenance so each entry can be promoted to `PluginHook`.

use wcore_plugin_api::registry::hooks::{HookPhase, HookRegistrar};

/// Borrows `&mut HostHookRegistrar` and stamps every `host_register_hook`
/// call with the originating plugin name. Mirrors the `ToolCaptureFor`
/// pattern from `tool_registrar.rs`.
pub struct HookCaptureFor<'a> {
    plugin: String,
    inner: &'a mut HostHookRegistrar,
}

impl<'a> HookRegistrar for HookCaptureFor<'a> {
    fn host_register_hook(&mut self, phase: HookPhase, name: String) -> Result<(), String> {
        self.inner
            .registered
            .push((self.plugin.clone(), phase, name));
        Ok(())
    }
}

/// Stores `(plugin, phase, name)` triples so `initialize_all` can promote
/// each entry to a `PluginHook { plugin, phase, name }`.
#[derive(Debug, Default)]
pub struct HostHookRegistrar {
    pub registered: Vec<(String, HookPhase, String)>,
}

impl HostHookRegistrar {
    /// Per-plugin capture sub-view — mirrors `HostToolRegistrar::capture_for_plugin`.
    /// `runner.rs` mints one per plugin so `PluginHook.plugin` is accurate.
    pub fn capture_for_plugin(&mut self, plugin: String) -> HookCaptureFor<'_> {
        HookCaptureFor {
            plugin,
            inner: self,
        }
    }
}

impl HookRegistrar for HostHookRegistrar {
    /// Fallback impl (used by `ScopedHookRegistry::new` in tests that build
    /// the scoped registry directly against the bare host registrar). Plugin
    /// name is recorded as an empty string; prefer `capture_for_plugin` in
    /// production paths.
    fn host_register_hook(&mut self, phase: HookPhase, name: String) -> Result<(), String> {
        self.registered.push((String::new(), phase, name));
        Ok(())
    }
}
