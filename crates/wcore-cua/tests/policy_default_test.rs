//! Wave SC SECURITY MAJOR fix — `CuaPolicy::default()` must match the
//! serde `#[serde(default = "default_true")]` round-trip.
//!
//! Before the fix, `#[derive(Default)]` produced
//! `first_time_per_app_approval = false` while
//! `serde_json::from_str("{}")` produced `true`. Whichever code path
//! produced the policy decided whether the first-time gate was on,
//! and the two paths disagreed silently. This test pins the invariant.

use wcore_cua::CuaPolicy;

#[test]
fn default_matches_serde_empty_roundtrip() {
    let default = CuaPolicy::default();
    let parsed: CuaPolicy = serde_json::from_str("{}").unwrap();

    assert_eq!(
        default.require_approval_for_app,
        parsed.require_approval_for_app
    );
    assert_eq!(default.forbidden_apps, parsed.forbidden_apps);
    assert_eq!(default.forbidden_key_combos, parsed.forbidden_key_combos);
    assert_eq!(
        default.first_time_per_app_approval, parsed.first_time_per_app_approval,
        "first_time_per_app_approval must match between Default::default() and serde empty struct"
    );
    assert_eq!(default.plugin_id, parsed.plugin_id);
}

#[test]
fn default_first_time_per_app_is_true() {
    // The audit knob: users expect "the LLM has never automated this
    // app before — confirm with the user" to be ON by default.
    let p = CuaPolicy::default();
    assert!(
        p.first_time_per_app_approval,
        "CuaPolicy::default() must enable first_time_per_app_approval"
    );
}

#[test]
fn serde_empty_struct_first_time_per_app_is_true() {
    let p: CuaPolicy = serde_json::from_str("{}").unwrap();
    assert!(
        p.first_time_per_app_approval,
        "serde empty struct must enable first_time_per_app_approval"
    );
}

#[test]
fn permissive_explicitly_disables_first_time_gate() {
    // The `permissive()` constructor is the documented "tests + bare
    // baseline" baseline that opts OUT of the gate. Pin that contract
    // — anyone changing it has to also update the call sites that
    // rely on it (every existing wcore-cua test).
    let p = CuaPolicy::permissive();
    assert!(
        !p.first_time_per_app_approval,
        "CuaPolicy::permissive() must disable first_time_per_app_approval"
    );
}

#[test]
fn full_config_roundtrip_preserves_first_time_flag() {
    // Explicit `false` survives the serde roundtrip — the
    // `#[serde(default)]` attribute MUST NOT clobber an
    // explicitly-set field.
    let json = r#"{"first_time_per_app_approval": false}"#;
    let p: CuaPolicy = serde_json::from_str(json).unwrap();
    assert!(!p.first_time_per_app_approval);
    let json = r#"{"first_time_per_app_approval": true}"#;
    let p: CuaPolicy = serde_json::from_str(json).unwrap();
    assert!(p.first_time_per_app_approval);
}
