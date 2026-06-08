//! F.8 — when `Capabilities.computer_use = false` (host advertises no
//! display), the engine MUST refuse to register the CUA tool. The
//! adapter surfaces `CuaError::CapabilityDisabled` in that case;
//! plugin-shell propagation lives in `wayland-cua` and is exercised
//! by the plugin's own load test.

use wcore_cua::{CuaError, CuaToolSpecLocal, from_spec};

#[test]
fn refuses_to_register_when_computer_use_capability_off() {
    let spec = CuaToolSpecLocal {
        computer_use_advertised: false,
        ..CuaToolSpecLocal::default()
    };
    let r = from_spec(spec);
    assert!(
        matches!(r, Err(CuaError::CapabilityDisabled)),
        "expected CapabilityDisabled, got {}",
        r.err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "<no error>".into())
    );
}

#[test]
fn api_spec_translation_preserves_policy() {
    use wcore_cua::adapter::from_api_spec;
    use wcore_plugin_api::cua_spec::{CuaPolicySpec, CuaToolSpec as ApiCuaToolSpec};

    let api_spec = ApiCuaToolSpec {
        tool_namespace: "Cua".into(),
        policy: CuaPolicySpec {
            require_approval_for_app: vec!["Keychain Access".into()],
            forbidden_apps: vec!["1Password".into()],
            forbidden_key_combos: vec!["cmd+q+system".into()],
            first_time_per_app_approval: true,
        },
        redact_screenshots: true,
    };
    let local = from_api_spec(api_spec, true);
    assert_eq!(local.tool_namespace, "Cua");
    assert!(local.computer_use_advertised);
    assert!(local.redact_screenshots);
    assert_eq!(
        local.policy.require_approval_for_app,
        vec!["Keychain Access".to_string()]
    );
    assert_eq!(local.policy.forbidden_apps, vec!["1Password".to_string()]);
    assert!(local.policy.first_time_per_app_approval);
}
