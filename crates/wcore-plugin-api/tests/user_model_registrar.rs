//! v0.6.4 Task 2.1 — tests for `ScopedUserModelRegistry` (permission gate,
//! duplicate detection, host-side capture). Mirrors the patterns in
//! `scoped_registry_semantics.rs` for the other registrars.

use wcore_plugin_api::registry::user_models::{ScopedUserModelRegistry, UserModelRegistrar};
use wcore_plugin_api::{PluginError, PluginManifest, UserModelSpec};

struct CaptureUserModels {
    registered: Vec<UserModelSpec>,
}

impl UserModelRegistrar for CaptureUserModels {
    fn host_register_user_model(&mut self, spec: UserModelSpec) -> Result<(), String> {
        if self.registered.iter().any(|s| s.name == spec.name) {
            return Err(format!("duplicate user_model: {}", spec.name));
        }
        self.registered.push(spec);
        Ok(())
    }
}

fn user_models_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-honcho"
version = "1.0.0"
description = "t"
entry = "builtin:h"
authors = ["t"]
license = "MIT"
[permissions]
register_user_models = true
"#,
    )
    .expect("user_models test manifest")
}

fn empty_manifest(name: &str) -> PluginManifest {
    PluginManifest::from_toml_str(&format!(
        r#"
[plugin]
name = "{name}"
version = "1.0.0"
description = "t"
entry = "builtin:t"
authors = ["t"]
license = "MIT"
"#,
    ))
    .unwrap()
}

fn sample_spec(name: &str) -> UserModelSpec {
    UserModelSpec {
        name: name.into(),
        description: "honcho backend".into(),
        backend: "honcho".into(),
        base_url: None,
        api_key_env: Some("HONCHO_API_KEY".into()),
        config: serde_json::Value::Null,
    }
}

#[test]
fn scoped_user_models_with_permission_registers() {
    let m = user_models_manifest();
    let mut host = CaptureUserModels {
        registered: Vec::new(),
    };
    let mut scoped = ScopedUserModelRegistry::new(&m, &mut host).expect("scoped reg");
    scoped
        .register_user_model(sample_spec("honcho"))
        .expect("registered");
    assert_eq!(host.registered.len(), 1);
    assert_eq!(host.registered[0].name, "honcho");
    assert_eq!(host.registered[0].backend, "honcho");
    assert_eq!(
        host.registered[0].api_key_env.as_deref(),
        Some("HONCHO_API_KEY")
    );
}

#[test]
fn scoped_user_models_constructor_rejects_without_permission() {
    let m_no = empty_manifest("wayland-mute");
    let mut host = CaptureUserModels {
        registered: Vec::new(),
    };
    let result = ScopedUserModelRegistry::new(&m_no, &mut host);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("must reject"),
    };
    assert!(matches!(err, PluginError::PermissionDenied { .. }));
}

#[test]
fn scoped_user_models_rejects_duplicate() {
    let m = user_models_manifest();
    let mut host = CaptureUserModels {
        registered: Vec::new(),
    };
    let mut scoped = ScopedUserModelRegistry::new(&m, &mut host).expect("scoped reg");
    scoped
        .register_user_model(sample_spec("honcho"))
        .expect("first");
    let err = scoped
        .register_user_model(sample_spec("honcho"))
        .expect_err("dup");
    assert!(matches!(err, PluginError::DuplicateRegistration { .. }));
}
