//! `ScopedExecutionBackendRegistry` — plugin-facing execution-backend
//! registration.
//!
//! Copied from `registry/browser.rs` on purpose. Same permission gate, same
//! at-most-one-per-plugin rule, same host-translates-after-initialize shape.
//! An execution backend is strictly more powerful than a browser tool, so it
//! gets no weaker a gate.

use crate::access_gate::PluginAccessGate;
use crate::error::{PluginError, PluginResult};
use crate::execution_backend_spec::ExecutionBackendSpec;
use crate::manifest::PluginManifest;

/// Host-side trait the wcore-agent adapter implements. It receives the
/// already-validated spec; translation into a real backend happens after
/// `initialize()` returns, so a plugin never holds a live backend handle
/// during its own construction.
pub trait ExecutionBackendRegistrar: Send {
    fn host_register(&mut self, spec: ExecutionBackendSpec) -> Result<(), String>;
}

pub struct ScopedExecutionBackendRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn ExecutionBackendRegistrar,
    registered: bool,
}

impl<'a> ScopedExecutionBackendRegistry<'a> {
    pub fn new(
        manifest: &PluginManifest,
        host: &'a mut dyn ExecutionBackendRegistrar,
    ) -> PluginResult<Self> {
        // An execution backend is a tool surface, so it reuses the standard
        // tools gate and its permission rules stay 1:1 with every other
        // registry rather than inventing a softer one.
        PluginAccessGate::require_tools(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: false,
        })
    }

    pub fn register_execution_backend(&mut self, spec: ExecutionBackendSpec) -> PluginResult<()> {
        if self.registered {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "execution_backend",
                name: spec.backend_id.clone(),
            });
        }
        // The HOST validates. A plugin that validated itself would be trusted
        // to decide whether it may shadow `local`.
        if let Some(reason) = spec.validation_error() {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "execution_backend",
                name: format!("{} (rejected: {reason})", spec.backend_id),
            });
        }
        self.host
            .host_register(spec.clone())
            .map_err(|e| PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "execution_backend",
                name: format!("{} ({e})", spec.backend_id),
            })?;
        self.registered = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_backend_spec::{
        ExecutionBackendKind, ExecutionLimitsSpec, ExecutionSecretChannel,
    };
    use crate::manifest::{PluginInfo, PluginPermissions};

    struct Capture {
        seen: Vec<ExecutionBackendSpec>,
    }

    impl ExecutionBackendRegistrar for Capture {
        fn host_register(&mut self, spec: ExecutionBackendSpec) -> Result<(), String> {
            self.seen.push(spec);
            Ok(())
        }
    }

    fn manifest(register_tools: bool) -> PluginManifest {
        PluginManifest {
            plugin: PluginInfo {
                name: "wayland-vendor-exec".into(),
                version: "0.1.0".into(),
                description: "test".into(),
                entry: Some("builtin:wayland_vendor_exec".into()),
                authors: vec![],
                license: "MIT".into(),
                deferred: false,
            },
            permissions: PluginPermissions {
                register_tools,
                tool_namespace: register_tools.then(|| "VendorExec".to_string()),
                ..Default::default()
            },
            capabilities: Default::default(),
            plugin_api_version: None,
            runtime: None,
            hooks: vec![],
            mcp_server: None,
        }
    }

    fn spec(backend_id: &str) -> ExecutionBackendSpec {
        ExecutionBackendSpec {
            backend_id: backend_id.into(),
            kind: ExecutionBackendKind::Cloud,
            version: "0.1.0".into(),
            limits: ExecutionLimitsSpec {
                cpu_millis: 1000,
                memory_bytes: 1 << 20,
                wall_time_ms: 1000,
                output_bytes: 1 << 16,
            },
            supports_artifact_transfer: true,
            supports_cancellation: true,
            supports_hibernation: false,
            secret_channel: ExecutionSecretChannel::VendorManaged,
            credential_env: vec![],
        }
    }

    #[test]
    fn register_once_succeeds_and_captures_the_spec() {
        let manifest = manifest(true);
        let mut host = Capture { seen: vec![] };
        {
            let mut registry =
                ScopedExecutionBackendRegistry::new(&manifest, &mut host).expect("gate opens");
            registry
                .register_execution_backend(spec("my-vendor"))
                .expect("first registration");
        }
        assert_eq!(host.seen.len(), 1);
        assert_eq!(host.seen[0].backend_id, "my-vendor");
    }

    #[test]
    fn a_second_registration_from_the_same_plugin_is_refused() {
        let manifest = manifest(true);
        let mut host = Capture { seen: vec![] };
        let mut registry =
            ScopedExecutionBackendRegistry::new(&manifest, &mut host).expect("gate opens");
        registry
            .register_execution_backend(spec("my-vendor"))
            .expect("first registration");
        let second = registry.register_execution_backend(spec("my-other-vendor"));
        assert!(matches!(
            second,
            Err(PluginError::DuplicateRegistration { .. })
        ));
    }

    #[test]
    fn a_plugin_without_the_tools_permission_cannot_register_a_backend_at_all() {
        let manifest = manifest(false);
        let mut host = Capture { seen: vec![] };
        assert!(ScopedExecutionBackendRegistry::new(&manifest, &mut host).is_err());
    }

    #[test]
    fn the_host_rejects_a_spec_that_would_shadow_a_built_in_backend() {
        let manifest = manifest(true);
        let mut host = Capture { seen: vec![] };
        let mut registry =
            ScopedExecutionBackendRegistry::new(&manifest, &mut host).expect("gate opens");
        assert!(registry.register_execution_backend(spec("local")).is_err());
        assert!(
            host.seen.is_empty(),
            "a rejected spec must never reach the host registrar"
        );
    }
}
