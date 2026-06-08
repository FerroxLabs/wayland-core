//! W8c.3 H.2 — verify `build_capabilities_with_plugins` flips the
//! per-plugin capability flags (`browser_suite`, `computer_use`,
//! `plugins`) when the matching plugin shells have loaded.
//!
//! `PluginCapabilitySet::from_loaded(...)` is the single source of
//! truth for the name → flag mapping; this test pins the mapping.

use std::sync::Arc;

use wcore_agent::output::protocol_sink::{PluginCapabilitySet, ProtocolSink};
use wcore_config::compat::ProviderCompat;
use wcore_config::tools::AdvertisedCapabilitiesConfig;
use wcore_protocol::writer::ProtocolWriter;

fn sink() -> ProtocolSink {
    ProtocolSink::new(Arc::new(ProtocolWriter::new()))
}

#[test]
fn browser_suite_advertised_when_wayland_browser_loaded() {
    let names = vec!["wayland-browser".to_string()];
    let caps = PluginCapabilitySet::from_loaded(&names);
    assert!(caps.browser_suite);
    assert!(!caps.computer_use);

    let compat = ProviderCompat::default();
    let advertised = AdvertisedCapabilitiesConfig::default();
    let advertised_caps =
        sink().build_capabilities_with_plugins(&compat, false, "default", true, &caps, &advertised);
    assert!(advertised_caps.browser_suite);
    assert!(advertised_caps.plugins);
    assert!(!advertised_caps.computer_use);
}

#[test]
fn computer_use_advertised_when_wayland_cua_loaded() {
    let names = vec!["wayland-cua".to_string()];
    let caps = PluginCapabilitySet::from_loaded(&names);
    assert!(caps.computer_use);
    assert!(!caps.browser_suite);

    let compat = ProviderCompat::default();
    let advertised = AdvertisedCapabilitiesConfig::default();
    let advertised_caps =
        sink().build_capabilities_with_plugins(&compat, false, "default", true, &caps, &advertised);
    assert!(advertised_caps.computer_use);
    assert!(advertised_caps.plugins);
    assert!(!advertised_caps.browser_suite);
}

#[test]
fn both_capabilities_flip_when_both_plugins_loaded() {
    let names = vec![
        "wayland-browser".to_string(),
        "wayland-cua".to_string(),
        "wayland-ijfw".to_string(),
    ];
    let caps = PluginCapabilitySet::from_loaded(&names);
    assert!(caps.browser_suite);
    assert!(caps.computer_use);

    let compat = ProviderCompat::default();
    let advertised = AdvertisedCapabilitiesConfig::default();
    let advertised_caps =
        sink().build_capabilities_with_plugins(&compat, false, "default", true, &caps, &advertised);
    assert!(advertised_caps.browser_suite);
    assert!(advertised_caps.computer_use);
    assert!(advertised_caps.plugins);
}

#[test]
fn no_capabilities_flip_with_empty_plugin_list() {
    let names: Vec<String> = Vec::new();
    let caps = PluginCapabilitySet::from_loaded(&names);
    assert!(!caps.browser_suite);
    assert!(!caps.computer_use);

    let compat = ProviderCompat::default();
    let advertised = AdvertisedCapabilitiesConfig::default();
    // has_plugins=false matches the empty-list case.
    let advertised_caps = sink().build_capabilities_with_plugins(
        &compat,
        false,
        "default",
        false,
        &caps,
        &advertised,
    );
    assert!(!advertised_caps.browser_suite);
    assert!(!advertised_caps.computer_use);
    assert!(!advertised_caps.plugins);
}

#[test]
fn unknown_plugin_names_do_not_flip_known_flags() {
    let names = vec!["wayland-totally-unknown".to_string()];
    let caps = PluginCapabilitySet::from_loaded(&names);
    assert!(!caps.browser_suite);
    assert!(!caps.computer_use);
}

#[test]
fn build_capabilities_back_compat_default_set_is_all_off() {
    // The original `build_capabilities` shim must keep emitting
    // browser_suite=false / computer_use=false so older callers stay
    // byte-identical on the wire.
    let compat = ProviderCompat::default();
    let advertised = AdvertisedCapabilitiesConfig::default();
    let advertised_caps = sink().build_capabilities(&compat, false, "default", true, &advertised);
    assert!(!advertised_caps.browser_suite);
    assert!(!advertised_caps.computer_use);
    assert!(advertised_caps.plugins);
}
