//! `ScopedUserModelRegistry` — plugin-facing user-model backend registration.
//!
//! Mirrors `ScopedRuleRegistry` / `ScopedMcpRegistry`: a permission gate
//! (`register_user_models`), a `Registrar` trait the host adapter
//! implements, and a `Scoped*Registry` constructor handed to the plugin via
//! `PluginContext`. Plain-data spec; reification into a live backend client
//! happens host-side in Task 2.2.
//!
//! Duplicate detection is keyed on `UserModelSpec.name` within a single
//! plugin's scope; the host adapter additionally enforces cross-plugin
//! uniqueness.

use crate::access_gate::PluginAccessGate;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;
use crate::user_model_spec::UserModelSpec;

/// Host-side trait the wcore-agent adapter implements. Receives the
/// plain-data `UserModelSpec`; the host adapter reifies into a backend
/// client. Nothing here names `wayland-honcho` — that lives in the host.
pub trait UserModelRegistrar: Send {
    /// Register one user-model spec. Returns `Err` on host-side duplicate
    /// (the host enforces cross-plugin uniqueness on `spec.name`).
    fn host_register_user_model(&mut self, spec: UserModelSpec) -> Result<(), String>;
}

/// Plugin-facing user-model registration.
pub struct ScopedUserModelRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn UserModelRegistrar,
    registered: Vec<String>,
}

impl<'a> ScopedUserModelRegistry<'a> {
    pub fn new(
        manifest: &PluginManifest,
        host: &'a mut dyn UserModelRegistrar,
    ) -> PluginResult<Self> {
        PluginAccessGate::require_user_models(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: Vec::new(),
        })
    }

    /// Register a user-model backend spec. Bare name is read from
    /// `spec.name`. Rejects duplicates inside the plugin's own scope
    /// before consulting the host.
    pub fn register_user_model(&mut self, spec: UserModelSpec) -> PluginResult<()> {
        let name = spec.name.clone();
        if self.registered.contains(&name) {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "user_model",
                name,
            });
        }
        self.host.host_register_user_model(spec).map_err(|e| {
            PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "user_model",
                name: format!("{name} ({e})"),
            }
        })?;
        self.registered.push(name);
        Ok(())
    }
}
