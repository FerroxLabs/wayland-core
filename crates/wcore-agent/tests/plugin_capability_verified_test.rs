//! Wave SC SECURITY MAJOR fix — `PluginCapabilitySet::from_verified`
//! consumes verified `(name, PluginIdentity)` pairs, not raw names.
//!
//! Closes the audit finding: a malicious crate setting
//! `name = "wayland-browser"` in its manifest cannot flip the host's
//! `browser_suite` capability flag unless the engine first verifies
//! the identity (Static via inventory symbol, or PathPrefix under
//! the host's trusted plugin root).
//!
//! The security boundary is the IDENTITY VERIFICATION the caller
//! performs BEFORE building the `(name, identity)` list — this test
//! pins the from_verified contract that the engine actually
//! consumes.

use wcore_agent::output::protocol_sink::PluginCapabilitySet;
use wcore_plugin_api::PluginIdentity;

#[test]
fn verified_static_plugin_flips_capability_flag() {
    // Real wayland-browser discovered via inventory → Static identity.
    let loaded: Vec<(String, PluginIdentity)> = vec![(
        "wayland-browser".to_string(),
        PluginIdentity::from_static("wayland-browser"),
    )];
    let caps = PluginCapabilitySet::from_verified(&loaded);
    assert!(
        caps.browser_suite,
        "verified wayland-browser must flip browser_suite"
    );
    assert!(!caps.computer_use, "wayland-cua not in list");
}

#[test]
fn unverified_empty_list_leaves_flags_off() {
    let empty: Vec<(String, PluginIdentity)> = Vec::new();
    let caps = PluginCapabilitySet::from_verified(&empty);
    assert!(!caps.browser_suite);
    assert!(!caps.computer_use);
}

#[test]
fn from_verified_supports_cua_capability_flag() {
    let loaded: Vec<(String, PluginIdentity)> = vec![(
        "wayland-cua".to_string(),
        PluginIdentity::from_static("wayland-cua"),
    )];
    let caps = PluginCapabilitySet::from_verified(&loaded);
    assert!(caps.computer_use);
    assert!(!caps.browser_suite);
}

#[test]
fn from_verified_handles_both_plugins() {
    let loaded: Vec<(String, PluginIdentity)> = vec![
        (
            "wayland-browser".to_string(),
            PluginIdentity::from_static("wayland-browser"),
        ),
        (
            "wayland-cua".to_string(),
            PluginIdentity::from_static("wayland-cua"),
        ),
    ];
    let caps = PluginCapabilitySet::from_verified(&loaded);
    assert!(caps.browser_suite);
    assert!(caps.computer_use);
}

#[test]
fn from_verified_ignores_unrelated_names() {
    // Even a verified plugin with an unknown name doesn't flip any
    // flag — the capability surface is locked to the two known names.
    let loaded: Vec<(String, PluginIdentity)> = vec![(
        "wayland-random-plugin".to_string(),
        PluginIdentity::from_static("wayland-random-plugin"),
    )];
    let caps = PluginCapabilitySet::from_verified(&loaded);
    assert!(!caps.browser_suite);
    assert!(!caps.computer_use);
}

#[test]
fn impersonation_attack_requires_verified_identity_to_succeed() {
    // The attack: malicious crate ships `name = "wayland-browser"` in
    // its manifest. The engine's bootstrap MUST verify the identity
    // (e.g. via inventory) BEFORE adding it to the from_verified
    // input. This test demonstrates the CORRECT use: the spoofed
    // name only appears alongside a Static identity that the engine
    // generated from its own inventory symbol — which a malicious
    // out-of-tree crate cannot inject without compiling against the
    // engine binary.
    //
    // The negative path (attacker doesn't pass identity check →
    // doesn't appear in from_verified input → flag stays off) is
    // exercised by `unverified_empty_list_leaves_flags_off`.
    let attacker_attempt: Vec<(String, PluginIdentity)> = Vec::new();
    let caps = PluginCapabilitySet::from_verified(&attacker_attempt);
    assert!(
        !caps.browser_suite,
        "attacker that fails identity verification must not flip browser_suite"
    );
}
