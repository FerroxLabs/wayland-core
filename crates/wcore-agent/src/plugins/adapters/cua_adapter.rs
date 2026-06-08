//! Host-side CUA adapter — wires a `wcore-plugin-api`-mirrored
//! `CuaToolSpec` (registered by a plugin shell such as `wayland-cua`)
//! into a concrete `wcore_cua::CuaTool` and stages it for the engine's
//! tool dispatcher.
//!
//! Mirrors the shape of the (parallel-wave) `browser_adapter.rs`: the
//! plugin layer cannot construct a `CuaTool` directly (REV-2 audit F2 —
//! plugins may not depend on `wcore-cua`), so this adapter sits in
//! `wcore-agent` which DOES depend on `wcore-cua`, picks up the
//! `CuaToolSpec` payload via the `CuaToolRegistrar` trait, and reifies
//! it into a `CuaTool` through `wcore_cua::adapter::from_api_spec` +
//! `from_spec`.
//!
//! **Audit F7 positive invariance.** `from_spec` consults
//! `wcore_cua::backends::linux_wayland::compositor_allows_background_input`
//! at the time of reification and refuses to mint a tool on restricted
//! Wayland compositors. This adapter surfaces that as a typed
//! registration failure rather than silently falling back.

use std::sync::Arc;

use wcore_cua::adapter::{CuaToolSpecLocal, from_api_spec, from_spec};
use wcore_cua::error::CuaError;
use wcore_cua::tool::CuaTool;
use wcore_plugin_api::cua_spec::CuaToolSpec;
use wcore_plugin_api::registry::cua::CuaToolRegistrar;

/// Outcome of a host-side `CuaToolSpec` registration. (No `Debug`
/// derive — `CuaTool` doesn't implement `Debug` since its `Box<dyn
/// ComputerUseBackend>` field can't.)
pub struct RegisteredCuaTool {
    pub plugin: String,
    pub tool_namespace: String,
    pub tool: Arc<CuaTool>,
}

/// Host-side collector — accepts `CuaToolSpec` payloads from plugins
/// through the `CuaToolRegistrar` trait, captures the originating
/// plugin name + the spec, and exposes a `reify_all` method that the
/// engine boot path calls after `Plugin::initialize` returns.
///
/// Splitting registration (sync, infallible from the plugin's view)
/// from reification (sync, fallible — may produce `WaylandRestricted`
/// or `CapabilityDisabled`) keeps the plugin-init phase deterministic
/// while still surfacing real per-platform errors at the boot stage
/// where the host can react.
pub struct HostCuaRegistrar {
    /// Whether the host has advertised `Capabilities.computer_use`.
    /// `from_spec` consults this — when `false`, every reification
    /// returns `CuaError::CapabilityDisabled`.
    computer_use_advertised: bool,
    /// (plugin_name, spec) pairs collected during plugin init.
    collected: Vec<(String, CuaToolSpec)>,
}

impl Default for HostCuaRegistrar {
    /// Capability gating defaults to `false` — until the host explicitly
    /// advertises `computer_use`, every reified `CuaTool` surfaces
    /// `CuaError::CapabilityDisabled`. Wire via
    /// `PluginRunner::with_computer_use_advertised(true)` once the
    /// host knows it is supported.
    fn default() -> Self {
        Self::new(false)
    }
}

impl HostCuaRegistrar {
    pub fn new(computer_use_advertised: bool) -> Self {
        Self {
            computer_use_advertised,
            collected: Vec::new(),
        }
    }

    pub fn computer_use_advertised(&self) -> bool {
        self.computer_use_advertised
    }

    pub fn set_computer_use_advertised(&mut self, advertised: bool) {
        self.computer_use_advertised = advertised;
    }

    /// Number of `CuaToolSpec`s captured so far.
    pub fn registered_count(&self) -> usize {
        self.collected.len()
    }

    pub fn capture_for_plugin(&mut self, plugin: impl Into<String>) -> CuaCaptureFor<'_> {
        CuaCaptureFor {
            inner: self,
            plugin: plugin.into(),
        }
    }

    /// Reify every collected `CuaToolSpec` into a real `CuaTool`.
    /// Returns the successful registrations and the per-plugin errors
    /// separately so the host can log the failures without aborting
    /// the whole boot.
    pub fn reify_all(self) -> (Vec<RegisteredCuaTool>, Vec<(String, CuaError)>) {
        let mut ok = Vec::new();
        let mut errs = Vec::new();
        for (plugin, spec) in self.collected {
            let tool_namespace = spec.tool_namespace.clone();
            let local: CuaToolSpecLocal = from_api_spec(spec, self.computer_use_advertised);
            // Override the policy plugin_id with the originating plugin
            // name so the persistent seen-apps store keys correctly per
            // plugin (SECURITY MAJOR fix from wave SC).
            let mut local = local;
            local.policy.plugin_id = plugin.clone();
            match from_spec(local) {
                Ok(tool) => ok.push(RegisteredCuaTool {
                    plugin,
                    tool_namespace,
                    tool,
                }),
                Err(e) => errs.push((plugin, e)),
            }
        }
        (ok, errs)
    }
}

/// Per-plugin capture sub-view. Implements `CuaToolRegistrar` so the
/// `ScopedCuaRegistry` for a single plugin can register through it.
pub struct CuaCaptureFor<'a> {
    inner: &'a mut HostCuaRegistrar,
    plugin: String,
}

impl<'a> CuaToolRegistrar for CuaCaptureFor<'a> {
    fn host_register(&mut self, spec: CuaToolSpec) -> Result<(), String> {
        self.inner.collected.push((self.plugin.clone(), spec));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn fixture_spec(namespace: &str) -> CuaToolSpec {
        CuaToolSpec {
            tool_namespace: namespace.into(),
            policy: Default::default(),
            redact_screenshots: false,
        }
    }

    fn clear_wayland_env() {
        unsafe {
            std::env::remove_var("WCORE_CUA_TEST_WAYLAND_PERMISSIVE");
            std::env::remove_var("WCORE_CUA_TEST_WAYLAND_RESTRICTED");
            std::env::remove_var("WAYLAND_DISPLAY");
        }
    }

    #[test]
    fn capability_disabled_propagates_through_adapter() {
        let mut reg = HostCuaRegistrar::new(/*advertised=*/ false);
        {
            let mut cap = reg.capture_for_plugin("wayland-cua");
            cap.host_register(fixture_spec("Cua")).unwrap();
        }
        let (ok, errs) = reg.reify_all();
        assert!(ok.is_empty(), "no-cap → no real tool");
        assert_eq!(errs.len(), 1);
        assert!(matches!(errs[0].1, CuaError::CapabilityDisabled));
    }

    /// Audit F7: on macOS / Windows (non-Wayland targets), reification
    /// succeeds when capability is advertised.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    #[serial]
    fn reifies_real_cua_tool_on_non_linux() {
        clear_wayland_env();
        let mut reg = HostCuaRegistrar::new(true);
        {
            let mut cap = reg.capture_for_plugin("wayland-cua");
            cap.host_register(fixture_spec("Cua")).unwrap();
        }
        let (ok, errs) = reg.reify_all();
        assert!(
            errs.is_empty(),
            "expected reification success, got: {errs:?}"
        );
        assert_eq!(ok.len(), 1);
        use wcore_tools::Tool;
        assert_eq!(ok[0].tool.name(), "Cua");
        assert_eq!(ok[0].plugin, "wayland-cua");
    }

    /// Audit F7 on Linux Wayland: restricted compositor refuses
    /// registration through the adapter.
    #[cfg(target_os = "linux")]
    #[test]
    #[serial]
    fn restricted_wayland_compositor_refuses_through_adapter() {
        clear_wayland_env();
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-test");
            std::env::set_var("WCORE_CUA_TEST_WAYLAND_RESTRICTED", "1");
        }
        let mut reg = HostCuaRegistrar::new(true);
        {
            let mut cap = reg.capture_for_plugin("wayland-cua");
            cap.host_register(fixture_spec("Cua")).unwrap();
        }
        let (ok, errs) = reg.reify_all();
        clear_wayland_env();
        assert!(ok.is_empty());
        assert_eq!(errs.len(), 1);
        assert!(matches!(errs[0].1, CuaError::WaylandRestricted { .. }));
    }

    /// Plugin-id propagation: the `policy.plugin_id` carried into the
    /// real `CuaPolicy` matches the plugin that registered the spec.
    /// Critical for the persistent seen-apps store keying (wave SC).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    #[serial]
    fn plugin_id_propagates_to_real_policy() {
        clear_wayland_env();
        let mut reg = HostCuaRegistrar::new(true);
        {
            let mut cap = reg.capture_for_plugin("custom-cua-plugin");
            cap.host_register(fixture_spec("CustomCua")).unwrap();
        }
        let (ok, _errs) = reg.reify_all();
        assert_eq!(ok[0].tool.policy().plugin_id, "custom-cua-plugin");
    }
}
