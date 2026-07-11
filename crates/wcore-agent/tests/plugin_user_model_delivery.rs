//! v0.6.4 Task 2.2 — user-model delivery via `apply_initialize_outcome`.
//!
//! Proves: given an `InitializeOutcome` carrying one or more
//! `CapturedUserModel` entries, `apply_initialize_outcome` returns an
//! `AppliedPluginCapabilities` whose `plugin_user_models` field carries those
//! captures forward verbatim. The carrier is consumed by Task 2.3
//! (`wayland-honcho` reifies it into a live `UserModel` client during
//! bootstrap).
//!
//! No live HTTP / network — pure in-memory data shuttling.

use serial_test::serial;
use wcore_agent::plugins::runner::CapturedUserModel;
use wcore_agent::plugins::{InitializeOutcome, apply_initialize_outcome};
use wcore_plugin_api::UserModelSpec;
use wcore_tools::registry::ToolRegistry;

fn spec(name: &str) -> UserModelSpec {
    UserModelSpec {
        name: name.to_string(),
        description: format!("{name} user-model"),
        backend: "honcho".to_string(),
        base_url: None,
        api_key_env: None,
        config: serde_json::Value::Null,
    }
}

fn captured(plugin: &str, name: &str) -> CapturedUserModel {
    CapturedUserModel {
        plugin: plugin.to_string(),
        spec: spec(name),
    }
}

/// A single captured user-model survives `apply_initialize_outcome` and lands
/// in `applied.plugin_user_models` with both `plugin` provenance and `spec`
/// intact.
#[test]
fn captured_user_model_survives_apply() {
    let mut outcome = InitializeOutcome::default();
    outcome
        .user_models
        .push(captured("wayland-honcho", "honcho"));

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert_eq!(
        applied.plugin_user_models.len(),
        1,
        "exactly one CapturedUserModel must reach applied.plugin_user_models"
    );
    let got = &applied.plugin_user_models[0];
    assert_eq!(got.plugin, "wayland-honcho");
    assert_eq!(got.spec.name, "honcho");
    assert_eq!(got.spec.backend, "honcho");
    assert_eq!(got.spec.description, "honcho user-model");
}

/// Multiple captures from different plugins all reach the carrier in order.
#[test]
fn multiple_captured_user_models_preserved() {
    let mut outcome = InitializeOutcome::default();
    outcome.user_models.push(captured("plugin-a", "alpha"));
    outcome.user_models.push(captured("plugin-b", "beta"));
    outcome.user_models.push(captured("plugin-c", "gamma"));

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert_eq!(applied.plugin_user_models.len(), 3);
    assert_eq!(applied.plugin_user_models[0].plugin, "plugin-a");
    assert_eq!(applied.plugin_user_models[0].spec.name, "alpha");
    assert_eq!(applied.plugin_user_models[1].plugin, "plugin-b");
    assert_eq!(applied.plugin_user_models[1].spec.name, "beta");
    assert_eq!(applied.plugin_user_models[2].plugin, "plugin-c");
    assert_eq!(applied.plugin_user_models[2].spec.name, "gamma");
}

/// Empty outcome → empty `plugin_user_models` (default-constructed carrier
/// must not introduce phantom entries).
#[test]
fn empty_outcome_yields_empty_carrier() {
    let outcome = InitializeOutcome::default();

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert!(
        applied.plugin_user_models.is_empty(),
        "no user-models captured → carrier must be empty"
    );
}

// ----------------------------------------------------------------------
// v0.6.5 Task 1.5 — reify path coverage. These tests exercise
// `AppliedPluginCapabilities::plugin_reified_user_models` and
// `user_model_errors`, the carriers Task 1.5 adds on top of the existing
// `plugin_user_models` (CapturedUserModel) data-carrier above.
// ----------------------------------------------------------------------

use std::env;
use wcore_agent::plugins::apply::ReifiedUserModelBackend;
use wcore_plugin_api::PluginError;

fn honcho_spec_with_env(env_var: &str) -> UserModelSpec {
    UserModelSpec {
        name: "honcho-prod".to_string(),
        description: "live honcho via plugin spec".to_string(),
        backend: "honcho".to_string(),
        base_url: Some("https://example-honcho.test".to_string()),
        api_key_env: Some(env_var.to_string()),
        config: serde_json::Value::Null,
    }
}

/// `backend = "honcho"` reifies into a live `HonchoClient` via
/// `HonchoClient::from_spec`.
#[serial]
#[test]
fn honcho_user_model_reifies() {
    let key_var = "WAYLAND_TASK_1_5_HONCHO_KEY";
    // SAFETY: process-global env mutation. Test uses a unique key name
    // so concurrent tests cannot collide on it.
    unsafe {
        env::set_var(key_var, "test-key");
    }

    let mut outcome = InitializeOutcome::default();
    outcome.user_models.push(CapturedUserModel {
        plugin: "wayland-honcho".to_string(),
        spec: honcho_spec_with_env(key_var),
    });

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    unsafe {
        env::remove_var(key_var);
    }

    assert_eq!(
        applied.plugin_reified_user_models.len(),
        1,
        "honcho spec must produce exactly one reified backend"
    );
    assert!(
        applied.user_model_errors.is_empty(),
        "successful reify must produce zero errors; got {:?}",
        applied.user_model_errors
    );
    let reified = &applied.plugin_reified_user_models[0];
    assert_eq!(reified.plugin, "wayland-honcho");
    assert_eq!(reified.name, "honcho-prod");
    assert!(matches!(
        reified.backend,
        ReifiedUserModelBackend::Honcho(_)
    ));
}

/// Unknown backend tag → typed `PluginError::UnknownUserModelBackend`
/// (no panic, no silent skip), and the spec does NOT appear in the
/// reified carrier.
#[test]
fn unknown_backend_typed_error() {
    let mut outcome = InitializeOutcome::default();
    outcome.user_models.push(CapturedUserModel {
        plugin: "rogue-plugin".to_string(),
        spec: UserModelSpec {
            name: "made-up".to_string(),
            description: "fictitious backend".to_string(),
            backend: "fictitious".to_string(),
            base_url: None,
            api_key_env: None,
            config: serde_json::Value::Null,
        },
    });

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert!(
        applied.plugin_reified_user_models.is_empty(),
        "unknown backend must not reify"
    );
    assert_eq!(applied.user_model_errors.len(), 1);
    match &applied.user_model_errors[0] {
        PluginError::UnknownUserModelBackend { plugin, backend } => {
            assert_eq!(plugin, "rogue-plugin");
            assert_eq!(backend, "fictitious");
        }
        other => panic!("expected UnknownUserModelBackend, got {other:?}"),
    }
}

/// Bug #6: an absent `HONCHO_API_KEY` is the unconfigured-optional-feature
/// case. Honcho must degrade SILENTLY like every other optional integration
/// — no reified client AND no surfaced error (so boot emits no nagging WARN
/// about a key the user never set up). The local-backend fallback in
/// bootstrap covers the actual user-model context.
#[serial]
#[test]
fn honcho_missing_api_key_degrades_silently() {
    let key_var = "WAYLAND_TASK_1_5_DEFINITELY_UNSET_KEY";
    // Make extra sure it's not set from a prior test.
    // SAFETY: `#[serial]` serializes every env-mutating test in this binary.
    unsafe {
        env::remove_var(key_var);
    }

    let mut outcome = InitializeOutcome::default();
    outcome.user_models.push(CapturedUserModel {
        plugin: "wayland-honcho".to_string(),
        spec: honcho_spec_with_env(key_var),
    });

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );

    assert!(
        applied.plugin_reified_user_models.is_empty(),
        "missing api key must not produce a reified client"
    );
    assert!(
        applied.user_model_errors.is_empty(),
        "missing api key for an optional backend must NOT surface an error \
         (silent degrade — Bug #6); got {:?}",
        applied.user_model_errors
    );
}
