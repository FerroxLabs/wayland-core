//! F.8 — verify the plugin manifest parses, declares the right surface,
//! and the default CuaToolSpec is the api-crate mirror (NOT a
//! wcore-cua type).

use wayland_cua::{MANIFEST_TOML, WaylandCua, default_cua_spec};
use wcore_plugin_api::{Plugin, PluginManifest};

#[test]
fn manifest_parses_and_declares_register_tools() {
    let m = PluginManifest::from_toml_str(MANIFEST_TOML).expect("manifest must parse");
    assert_eq!(m.plugin.name, "wayland-cua");
    assert!(m.permissions.register_tools);
    assert_eq!(m.permissions.tool_namespace.as_deref(), Some("Cua"));
    assert!(!m.permissions.register_hooks);
    assert!(!m.permissions.register_providers);
    assert!(!m.permissions.register_agents);
    assert!(!m.permissions.register_skills);
    assert!(!m.permissions.register_rules);
    assert!(!m.permissions.register_mcp_server);
}

#[test]
fn default_spec_has_cua_namespace_and_first_time_approval() {
    let s = default_cua_spec();
    assert_eq!(s.tool_namespace, "Cua");
    assert!(s.policy.first_time_per_app_approval);
    assert!(s.policy.require_approval_for_app.is_empty());
    assert!(s.policy.forbidden_apps.is_empty());
    assert!(s.policy.forbidden_key_combos.is_empty());
    assert!(!s.redact_screenshots);
}

#[test]
fn plugin_manifest_round_trips_through_factory() {
    let plugin = WaylandCua;
    let m = plugin.manifest();
    assert_eq!(m.plugin.entry, "builtin:wayland_cua");
}

#[test]
fn cua_spec_round_trips_serde() {
    let spec = default_cua_spec();
    let s = serde_json::to_string(&spec).unwrap();
    let parsed: wcore_plugin_api::cua_spec::CuaToolSpec = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed.tool_namespace, spec.tool_namespace);
    assert_eq!(
        parsed.policy.first_time_per_app_approval,
        spec.policy.first_time_per_app_approval
    );
}
