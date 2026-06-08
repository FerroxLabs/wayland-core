//! E.13 — verify the plugin manifest parses, declares the right surface,
//! and the default BrowserToolSpec is the api-crate mirror (NOT a
//! wcore-browser type).

use wayland_browser::{MANIFEST_TOML, WaylandBrowser, default_browser_spec};
use wcore_plugin_api::browser_spec::BrowserProviderHint;
use wcore_plugin_api::{Plugin, PluginManifest};

#[test]
fn manifest_parses_and_declares_register_tools() {
    let m = PluginManifest::from_toml_str(MANIFEST_TOML).expect("manifest must parse");
    assert_eq!(m.plugin.name, "wayland-browser");
    assert!(m.permissions.register_tools);
    assert_eq!(m.permissions.tool_namespace.as_deref(), Some("Browser"));
    assert!(!m.permissions.register_hooks);
    assert!(!m.permissions.register_providers);
}

#[test]
fn default_spec_has_auto_provider_and_fail_closed_policy() {
    // v0.2.1 Wave SB: default policy flipped from "allow" (fail-open)
    // to "deny" (fail-closed). Operators must explicitly allow-list
    // origins for the plugin to make any request. See
    // STABILITY-v0.2.0.md MAJOR #6.
    let s = default_browser_spec();
    assert_eq!(s.tool_namespace, "Browser");
    assert_eq!(s.preferred_provider, BrowserProviderHint::Auto);
    assert_eq!(
        s.policy.default_action, "deny",
        "default policy must fail-closed (v0.2.1 contract)"
    );
    assert!(s.policy.allowed_origins.is_empty());
    assert!(s.policy.denied_origins.is_empty());
    assert!(!s.allow_cloud);
}

#[test]
fn plugin_manifest_round_trips_through_factory() {
    let plugin = WaylandBrowser;
    let m = plugin.manifest();
    assert_eq!(m.plugin.entry, "builtin:wayland_browser");
}
