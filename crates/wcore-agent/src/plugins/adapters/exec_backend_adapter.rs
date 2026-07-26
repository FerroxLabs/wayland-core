//! F25-01 — host execution-backend adapter.
//!
//! `HostExecutionBackendRegistrar` implements
//! `wcore_plugin_api::registry::execution_backends::ExecutionBackendRegistrar`.
//! When a plugin calls `register_execution_backend(spec)` inside its
//! `initialize()`, the host CAPTURES the spec here. Translation into a real
//! backend happens only AFTER `PluginRunner::initialize_all` returns, exactly
//! as `HostBrowserRegistrar` does.
//!
//! Audit F2: the plugin shell stays free of `wcore-exec-backend`; this crate
//! is where the real translation happens, and the `FORBIDDEN_CORE_IMPORTS`
//! build lint in `wcore-plugin-api` now names `wcore-exec-backend` so a plugin
//! cannot reach around the mirror.

use wcore_plugin_api::execution_backend_spec::ExecutionBackendSpec;
use wcore_plugin_api::registry::execution_backends::ExecutionBackendRegistrar;

/// Captures every `ExecutionBackendSpec` a plugin registers.
#[derive(Debug, Default)]
pub struct HostExecutionBackendRegistrar {
    pub specs: Vec<ExecutionBackendSpec>,
}

impl ExecutionBackendRegistrar for HostExecutionBackendRegistrar {
    fn host_register(&mut self, spec: ExecutionBackendSpec) -> Result<(), String> {
        // The scoped registry already validated, but the host re-validates
        // rather than trusting: the registry runs in the plugin's call stack
        // and a hostile plugin runtime could bypass it.
        if let Some(reason) = spec.validation_error() {
            return Err(format!(
                "execution backend spec '{}' rejected: {reason}",
                spec.backend_id
            ));
        }
        if self.specs.iter().any(|s| s.backend_id == spec.backend_id) {
            return Err(format!("duplicate execution backend id: {}", spec.backend_id));
        }
        self.specs.push(spec);
        Ok(())
    }
}

impl HostExecutionBackendRegistrar {
    /// Names of every plugin-declared backend, in registration order.
    ///
    /// Reification into a live `wcore_exec_backend::ExecutionBackend` is NOT
    /// done here and that is deliberate, not an oversight: a plugin-declared
    /// backend describes a transport the host has no implementation for, so
    /// reification needs a transport factory this phase does not build. The
    /// phase's four reference backends are built-in, and the mirror exists so
    /// the ISOLATION boundary is right when that factory lands. Recorded as an
    /// explicit gap rather than a stub that pretends to work.
    pub fn declared_backend_ids(&self) -> Vec<String> {
        self.specs.iter().map(|s| s.backend_id.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_plugin_api::execution_backend_spec::{
        ExecutionBackendKind, ExecutionLimitsSpec, ExecutionSecretChannel,
    };

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
    fn the_host_captures_a_valid_spec() {
        let mut host = HostExecutionBackendRegistrar::default();
        host.host_register(spec("my-vendor")).expect("captured");
        assert_eq!(host.declared_backend_ids(), vec!["my-vendor".to_string()]);
    }

    #[test]
    fn the_host_revalidates_rather_than_trusting_the_scoped_registry() {
        // A hostile plugin runtime that bypassed ScopedExecutionBackendRegistry
        // still cannot shadow a built-in backend.
        let mut host = HostExecutionBackendRegistrar::default();
        assert!(host.host_register(spec("local")).is_err());
        assert!(host.specs.is_empty());
    }

    #[test]
    fn two_plugins_cannot_claim_the_same_backend_id() {
        let mut host = HostExecutionBackendRegistrar::default();
        host.host_register(spec("my-vendor")).expect("first");
        assert!(host.host_register(spec("my-vendor")).is_err());
    }
}
