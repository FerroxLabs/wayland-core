//! `ExecutionBackendSpec` — the plugin-facing MIRROR of the F25-01 execution
//! backend contract.
//!
//! This crate cannot depend on `wcore-exec-backend`, and the
//! `FORBIDDEN_CORE_IMPORTS` build.rs lint now names it explicitly so it never
//! silently can. A plugin therefore DESCRIBES an execution backend
//! declaratively; the host adapter in `wcore-agent` (which is allowed to
//! depend on the real crate) translates the spec into a live backend AFTER
//! `initialize()` returns.
//!
//! The shape is copied from `browser_spec.rs` deliberately rather than
//! invented: the same isolation problem has the same solution, and a second
//! pattern for the same problem is a second thing to get wrong.

use serde::{Deserialize, Serialize};

/// Which transport family the plugin's backend belongs to. Mirrors
/// `wcore_exec_backend::BackendKind` without importing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackendKind {
    Local,
    Container,
    Ssh,
    Cloud,
    /// A transport family the core does not model. The host decides whether it
    /// admits one, and the default is that it does not.
    Other,
}

/// How the plugin's backend says secrets reach the executing task. Declared so
/// a host can refuse a channel it does not trust BEFORE anything runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSecretChannel {
    None,
    LocalProcessEnv,
    ContainerEnv,
    RemoteTransport,
    VendorManaged,
}

/// The resource ceiling a plugin backend declares it will accept. Mirrors the
/// contract's `ResourceBudget`, including its rule that a zero is invalid
/// rather than unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionLimitsSpec {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub wall_time_ms: u64,
    pub output_bytes: u64,
}

impl ExecutionLimitsSpec {
    pub fn is_valid(&self) -> bool {
        self.cpu_millis != 0
            && self.memory_bytes != 0
            && self.wall_time_ms != 0
            && self.output_bytes != 0
    }
}

/// A plugin's declarative description of an execution backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionBackendSpec {
    /// The operator-visible name (`wayland-core backend run --backend <id>`).
    pub backend_id: String,
    pub kind: ExecutionBackendKind,
    pub version: String,
    pub limits: ExecutionLimitsSpec,
    pub supports_artifact_transfer: bool,
    pub supports_cancellation: bool,
    /// A plugin claiming hibernation must be able to OBSERVE the transition,
    /// not merely request one — the host records an unobserved transition as
    /// unobserved regardless of what the spec claims.
    pub supports_hibernation: bool,
    pub secret_channel: ExecutionSecretChannel,
    /// Environment variable NAMES the backend reads for its own control-plane
    /// credential. Names only; a spec has nowhere to put a value.
    #[serde(default)]
    pub credential_env: Vec<String>,
}

impl ExecutionBackendSpec {
    /// Validation the HOST performs, not the plugin. A plugin cannot be
    /// trusted to validate its own declaration.
    pub fn validation_error(&self) -> Option<String> {
        if self.backend_id.is_empty() || self.backend_id.len() > 128 {
            return Some("backend_id must be 1..=128 bytes".into());
        }
        if !self
            .backend_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Some(
                "backend_id must be ascii alphanumeric, '-', '_' or '.' — it becomes a CLI \
                 argument and a state-directory filename"
                    .into(),
            );
        }
        if !self.limits.is_valid() {
            return Some("every limit field must be non-zero; a zero is invalid, not unlimited".into());
        }
        for name in &self.credential_env {
            if name.contains('=') || name.is_empty() {
                return Some(format!("credential_env entry '{name}' is not a variable NAME"));
            }
        }
        // A plugin must not be able to shadow a built-in reference backend and
        // silently receive work an operator meant for the real one.
        if matches!(
            self.backend_id.as_str(),
            "local" | "container" | "ssh" | "cloud"
        ) {
            return Some(format!(
                "backend_id '{}' is a built-in reference backend and cannot be shadowed by a plugin",
                self.backend_id
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            supports_hibernation: true,
            secret_channel: ExecutionSecretChannel::VendorManaged,
            credential_env: vec!["MY_VENDOR_TOKEN".into()],
        }
    }

    #[test]
    fn a_valid_spec_passes() {
        assert_eq!(spec("my-vendor").validation_error(), None);
    }

    #[test]
    fn a_plugin_cannot_shadow_a_built_in_reference_backend() {
        for reserved in ["local", "container", "ssh", "cloud"] {
            assert!(
                spec(reserved).validation_error().is_some(),
                "'{reserved}' must be refused"
            );
        }
    }

    #[test]
    fn a_backend_id_that_could_escape_a_path_or_an_argv_is_refused() {
        assert!(spec("../../etc").validation_error().is_some());
        assert!(spec("a b").validation_error().is_some());
        assert!(spec("--task").validation_error().is_some());
    }

    #[test]
    fn a_zero_limit_is_invalid_rather_than_unlimited() {
        let mut zeroed = spec("my-vendor");
        zeroed.limits.memory_bytes = 0;
        assert!(zeroed.validation_error().is_some());
    }

    #[test]
    fn a_credential_value_masquerading_as_a_name_is_refused() {
        let mut leaky = spec("my-vendor");
        leaky.credential_env = vec!["MY_VENDOR_TOKEN=fo1_secret".into()];
        assert!(leaky.validation_error().is_some());
    }
}
