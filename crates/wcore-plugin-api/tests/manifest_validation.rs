//! Tests for `PluginManifest` TOML loading and validation.

use wcore_plugin_api::error::PluginError;
use wcore_plugin_api::manifest::PluginManifest;

#[test]
fn manifest_round_trip_minimal() {
    let toml = r#"
[plugin]
name = "wayland-test"
version = "0.1.0"
description = "A test plugin"
entry = "builtin:wayland_test"
authors = ["test"]
license = "MIT"
"#;
    let m = PluginManifest::from_toml_str(toml).expect("parse minimal manifest");
    assert_eq!(m.plugin.name, "wayland-test");
    assert_eq!(m.plugin.version, "0.1.0");
    // permissions default to all-false:
    assert!(!m.permissions.register_tools);
    assert!(!m.permissions.register_hooks);
    assert!(!m.permissions.register_providers);
    assert!(!m.permissions.register_agents);
    assert!(!m.permissions.register_skills);
    assert!(!m.permissions.register_rules);
    assert!(!m.permissions.register_mcp_server);
}

#[test]
fn manifest_full_ijfw_shape() {
    let toml = r#"
[plugin]
name = "wayland-ijfw"
version = "1.3.1"
description = "IJFW unified cross-tool memory + workflows"
entry = "builtin:wcore_ijfw"
authors = ["Sean Donahoe"]
license = "MIT"

[permissions]
register_tools       = true
register_hooks       = true
register_providers   = false
register_agents      = true
register_skills      = true
register_rules       = true
register_mcp_server  = true
tool_namespace       = "ijfw"
memory_partitions_writable = ["P2"]
memory_partitions_readable = ["P2", "P3", "P4"]
mcp_servers_visible  = "self_only"

[capabilities]
required = ["structured_traces"]
optional = ["streaming_tools", "hitl_suspend"]
"#;
    let m = PluginManifest::from_toml_str(toml).expect("parse IJFW manifest");
    assert!(m.permissions.register_tools);
    assert!(m.permissions.register_hooks);
    assert!(m.permissions.register_mcp_server);
    assert_eq!(m.permissions.tool_namespace.as_deref(), Some("ijfw"));
    assert_eq!(
        m.permissions.memory_partitions_writable,
        vec!["P2".to_string()]
    );
    assert_eq!(
        m.permissions.mcp_servers_visible.as_deref(),
        Some("self_only")
    );
    assert_eq!(
        m.capabilities.required,
        vec!["structured_traces".to_string()]
    );
}

#[test]
fn manifest_rejects_register_tools_without_namespace() {
    let toml = r#"
[plugin]
name = "wayland-no-ns"
version = "0.1.0"
description = "missing namespace"
entry = "builtin:bad"
authors = ["test"]
license = "MIT"

[permissions]
register_tools = true
"#;
    let err = PluginManifest::from_toml_str(toml).expect_err("must reject");
    assert!(
        matches!(err, PluginError::ManifestSchema { .. }),
        "expected ManifestSchema error, got {err:?}"
    );
}

#[test]
fn manifest_rejects_invalid_partition_value() {
    let toml = r#"
[plugin]
name = "wayland-bad-p"
version = "0.1.0"
description = "bad partition"
entry = "builtin:bad"
authors = ["test"]
license = "MIT"

[permissions]
memory_partitions_readable = ["PX"]
"#;
    let err = PluginManifest::from_toml_str(toml).expect_err("must reject");
    assert!(matches!(err, PluginError::ManifestSchema { .. }));
}

#[test]
fn manifest_rejects_p5_writable() {
    let toml = r#"
[plugin]
name = "wayland-p5"
version = "0.1.0"
description = "tries to write P5"
entry = "builtin:bad"
authors = ["test"]
license = "MIT"

[permissions]
memory_partitions_writable = ["P5"]
"#;
    let err = PluginManifest::from_toml_str(toml).expect_err("must reject P5 writable");
    assert!(matches!(err, PluginError::ManifestSchema { .. }));
}

#[test]
fn manifest_rejects_malformed_toml() {
    let err = PluginManifest::from_toml_str("not valid toml [[[").expect_err("must reject");
    assert!(matches!(err, PluginError::ManifestParse { .. }));
}
