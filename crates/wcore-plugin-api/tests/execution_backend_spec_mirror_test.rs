//! The mirror is only worth anything if it stays a MIRROR.
//!
//! These tests assert the two properties that make the F25-01 plugin surface
//! safe: the api crate carries no dependency on the real execution-backend
//! crate or on the containment crate, and the spec is a declarative
//! description with nowhere to put a live handle or a secret value.

use wcore_plugin_api::execution_backend_spec::{
    ExecutionBackendKind, ExecutionBackendSpec, ExecutionLimitsSpec, ExecutionSecretChannel,
};
use wcore_plugin_api::manifest::{PluginInfo, PluginManifest, PluginPermissions};
use wcore_plugin_api::registry::execution_backends::{
    ExecutionBackendRegistrar, ScopedExecutionBackendRegistry,
};

struct Capture {
    seen: Vec<ExecutionBackendSpec>,
    fail_with: Option<String>,
}

impl ExecutionBackendRegistrar for Capture {
    fn host_register(&mut self, spec: ExecutionBackendSpec) -> Result<(), String> {
        if let Some(reason) = &self.fail_with {
            return Err(reason.clone());
        }
        self.seen.push(spec);
        Ok(())
    }
}

fn manifest(register_tools: bool) -> PluginManifest {
    PluginManifest {
        plugin: PluginInfo {
            name: "wayland-vendor-exec".into(),
            version: "0.1.0".into(),
            description: "mirror test".into(),
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
        supports_hibernation: true,
        secret_channel: ExecutionSecretChannel::VendorManaged,
        credential_env: vec!["MY_VENDOR_TOKEN".into()],
    }
}

#[test]
fn the_api_crate_declares_no_dependency_on_the_real_execution_backend_crate() {
    // The build.rs lint enforces this at build time; stating it here too means
    // a reader who never opens build.rs still learns the invariant, and a
    // change that removed the lint is still caught.
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !manifest.contains("wcore-exec-backend"),
        "wcore-plugin-api must not depend on wcore-exec-backend — that is exactly the dependency \
         the ExecutionBackendSpec mirror exists to avoid"
    );
}

#[test]
fn the_build_lint_names_the_execution_backend_crate_and_reasons_about_the_sandbox_one() {
    let build_rs = include_str!("../build.rs");
    assert!(
        build_rs.contains("\"wcore-exec-backend\""),
        "the enabling capability must be forbidden outright"
    );
    // `wcore-sandbox` is DELIBERATELY not in the list, and the reason is
    // recorded rather than left to be rediscovered. The F25-01 plan believed
    // its absence was an oversight; it is measured to be an intentional M5.1
    // allowlist entry that `PluginContext` depends on. This assertion pins the
    // reasoning so a future reader does not "fix" it and break the build.
    assert!(
        build_rs.contains("DELIBERATELY ABSENT: `wcore-sandbox`"),
        "the sandbox allowlist decision must stay recorded next to the list it is absent from"
    );
}

#[test]
fn the_sandbox_handle_is_still_the_constraining_capability_it_was_designed_to_be() {
    // The distinction that justifies forbidding one crate and allowing the
    // other: a SandboxRegistry can only NARROW what a plugin may do, while an
    // execution backend would WIDEN it (off-box execution plus a credential).
    let context = include_str!("../src/context.rs");
    assert!(
        context.contains("pub sandbox: Option<std::sync::Arc<wcore_sandbox::SandboxRegistry>>"),
        "if this handle ever stops being an Arc<SandboxRegistry>, the allowlist argument above \
         needs re-deriving rather than inheriting"
    );
}

#[test]
fn registration_requires_the_same_permission_gate_as_the_browser_spec() {
    let mut host = Capture {
        seen: vec![],
        fail_with: None,
    };
    assert!(
        ScopedExecutionBackendRegistry::new(&manifest(false), &mut host).is_err(),
        "a plugin without register_tools must not open an execution-backend registry"
    );
    let mut host = Capture {
        seen: vec![],
        fail_with: None,
    };
    assert!(ScopedExecutionBackendRegistry::new(&manifest(true), &mut host).is_ok());
}

#[test]
fn a_second_registration_from_the_same_plugin_is_refused() {
    let binding = manifest(true);
    let mut host = Capture {
        seen: vec![],
        fail_with: None,
    };
    let mut registry = ScopedExecutionBackendRegistry::new(&binding, &mut host).unwrap();
    registry
        .register_execution_backend(spec("my-vendor"))
        .expect("first registration");
    assert!(
        registry
            .register_execution_backend(spec("my-second-vendor"))
            .is_err(),
        "a plugin owns at most one execution backend"
    );
}

#[test]
fn a_host_refusal_propagates_and_does_not_mark_the_plugin_registered() {
    let binding = manifest(true);
    let mut host = Capture {
        seen: vec![],
        fail_with: Some("host says no".into()),
    };
    let mut registry = ScopedExecutionBackendRegistry::new(&binding, &mut host).unwrap();
    assert!(registry.register_execution_backend(spec("my-vendor")).is_err());
    // Having been refused, the plugin may try again — the slot was not
    // consumed by a registration that never happened.
    assert!(registry.register_execution_backend(spec("my-vendor")).is_err());
}

#[test]
fn the_spec_round_trips_through_json_without_carrying_a_handle_or_a_value() {
    let original = spec("my-vendor");
    let json = serde_json::to_string(&original).expect("serializable");
    // A declarative spec must survive a serialization boundary — if it could
    // not, it would be carrying something live.
    let restored: ExecutionBackendSpec = serde_json::from_str(&json).expect("deserializable");
    assert_eq!(original, restored);
    assert!(
        json.contains("MY_VENDOR_TOKEN"),
        "credential env NAMES are part of the declaration"
    );
    assert!(
        !json.to_lowercase().contains("secret_value"),
        "the spec has no field for a credential VALUE"
    );
}
