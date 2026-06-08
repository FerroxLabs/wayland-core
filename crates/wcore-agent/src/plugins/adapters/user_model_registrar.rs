//! v0.6.4 Task 2.1 — host-side capture for plugin-registered user-model
//! backends. Plain in-memory collector mirroring `HostRuleRegistrar` /
//! `HostMcpRegistrar`. The runner enriches each captured spec with the
//! originating plugin name into `CapturedUserModel`. Task 2.2 reifies into
//! a live backend client (e.g. `wayland_honcho::HonchoClient`).

use wcore_plugin_api::UserModelSpec;
use wcore_plugin_api::registry::user_models::UserModelRegistrar;

#[derive(Debug, Default)]
pub struct HostUserModelRegistrar {
    /// `(plugin, spec)` tuples; the originating plugin name is stamped by
    /// `capture_for_plugin` mirroring the tool/cua arm in `runner.rs`.
    pub registered: Vec<(String, UserModelSpec)>,
}

impl HostUserModelRegistrar {
    /// Per-plugin capture borrow — stamps each captured spec with the
    /// originating plugin name. Cross-plugin uniqueness is enforced on
    /// `spec.name`, matching the spec-only host duplicate semantics.
    pub fn capture_for_plugin(&mut self, plugin_name: String) -> HostUserModelRegistrarCapture<'_> {
        HostUserModelRegistrarCapture {
            plugin_name,
            owner: self,
        }
    }
}

/// Per-plugin capture handle (RAII borrow of `HostUserModelRegistrar`).
pub struct HostUserModelRegistrarCapture<'a> {
    plugin_name: String,
    owner: &'a mut HostUserModelRegistrar,
}

impl UserModelRegistrar for HostUserModelRegistrarCapture<'_> {
    fn host_register_user_model(&mut self, spec: UserModelSpec) -> Result<(), String> {
        if self
            .owner
            .registered
            .iter()
            .any(|(_, s)| s.name == spec.name)
        {
            return Err(format!("duplicate user_model: {}", spec.name));
        }
        self.owner.registered.push((self.plugin_name.clone(), spec));
        Ok(())
    }
}
