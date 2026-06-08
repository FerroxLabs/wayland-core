//! G.1 acceptance — verify the manifest parses, declares the full
//! register_* surface, and the factory wires through inventory.

use wayland_ijfw::{MANIFEST_TOML, WaylandIjfw};
use wcore_plugin_api::{Plugin, PluginManifest};

#[test]
fn manifest_parses_and_declares_full_surface() {
    let m = PluginManifest::from_toml_str(MANIFEST_TOML).expect("manifest must parse");
    assert_eq!(m.plugin.name, "wayland-ijfw");
    assert_eq!(m.plugin.entry, "builtin:wayland_ijfw");

    assert!(m.permissions.register_tools);
    assert_eq!(m.permissions.tool_namespace.as_deref(), Some("ijfw"));
    assert!(m.permissions.register_hooks);
    assert!(m.permissions.register_agents);
    assert!(m.permissions.register_skills);
    assert!(m.permissions.register_rules);
    assert!(m.permissions.register_mcp_server);

    // Providers are explicitly NOT granted — wayland-ollama owns that
    // surface. This boundary is asserted both here and in the full
    // surface test.
    assert!(!m.permissions.register_providers);

    // Memory partitions: P2 writable; P2/P3/P4 readable; P5 absent.
    assert_eq!(m.permissions.memory_partitions_writable, vec!["P2"]);
    assert_eq!(
        m.permissions.memory_partitions_readable,
        vec!["P2".to_string(), "P3".to_string(), "P4".to_string()]
    );
    assert!(
        !m.permissions
            .memory_partitions_readable
            .contains(&"P5".into())
    );
}

#[test]
fn plugin_struct_returns_static_manifest() {
    let p = WaylandIjfw;
    let m = p.manifest();
    assert_eq!(m.plugin.name, "wayland-ijfw");
}
