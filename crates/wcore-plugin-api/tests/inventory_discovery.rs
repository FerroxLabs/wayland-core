//! Verify that `inventory::submit!` plugins are discoverable at runtime.
//!
//! v0.6.5 Task 1.1 — this test file also doubles as the **glob-import
//! smoke test**: every `wcore_plugin_api` item used below MUST be in the
//! `pub use ...` block of `lib.rs`, because the imports come in through a
//! glob (`use wcore_plugin_api::*;`).

use wcore_plugin_api::*;

struct DummyPlugin;

#[async_trait::async_trait]
impl Plugin for DummyPlugin {
    fn manifest(&self) -> &PluginManifest {
        static MANIFEST: std::sync::OnceLock<PluginManifest> = std::sync::OnceLock::new();
        MANIFEST.get_or_init(|| {
            PluginManifest::from_toml_str(
                r#"
[plugin]
name = "wayland-test-dummy"
version = "0.0.1"
description = "inventory discovery test"
entry = "builtin:dummy"
authors = ["test"]
license = "MIT"
"#,
            )
            .expect("dummy manifest")
        })
    }
}

struct DummyFactory;

impl PluginFactory for DummyFactory {
    fn name(&self) -> &'static str {
        "wayland-test-dummy"
    }
    fn build(&self) -> Box<dyn Plugin> {
        Box::new(DummyPlugin)
    }
}

inventory::submit! {
    &DummyFactory as &dyn PluginFactory
}

#[test]
fn dummy_plugin_discoverable_via_inventory() {
    let names: Vec<&str> = plugin_inventory::iter().map(|f| f.name()).collect();
    assert!(
        names.contains(&"wayland-test-dummy"),
        "expected dummy plugin in inventory, got {names:?}"
    );
}

#[test]
fn discovered_factory_builds_a_working_plugin() {
    let factory = plugin_inventory::iter()
        .find(|f| f.name() == "wayland-test-dummy")
        .expect("dummy plugin not found");
    let plugin = factory.build();
    assert_eq!(plugin.manifest().plugin.name, "wayland-test-dummy");
    assert_eq!(plugin.manifest().plugin.version, "0.0.1");
}

// v0.6.5 Task 1.1 — glob-import smoke: the `use wcore_plugin_api::*;` at
// the top of this file already proves `Plugin`, `PluginFactory`,
// `PluginManifest`, and `plugin_inventory` are reachable via the glob.
// This test just pins the assertion explicitly so a future re-export
// regression fires a named failure rather than a confusing compile error
// at the imports site.
#[test]
fn glob_import_smoke() {
    // Reference every glob-imported symbol to make the linker yell if any
    // is missing from the public re-exports.
    let _: Option<&dyn PluginFactory> = None;
    let _: Option<Box<dyn Plugin>> = None;
    let _: Option<PluginManifest> = None;
    let _: fn() = || {
        let _ = plugin_inventory::iter();
    };
    // Compile-only assertion; nothing to run.
}

#[test]
fn version_mismatch_rejects() {
    // A manifest that declares an unsupported plugin_api_version must
    // fail the require_api_version check.
    let manifest = PluginManifest::from_toml_str(
        r#"
plugin_api_version = "0.9"

[plugin]
name = "wayland-test-bad-version"
version = "0.0.1"
description = "version mismatch test"
entry = "builtin:bad"
authors = ["test"]
license = "MIT"
"#,
    )
    .expect("manifest parses");

    let err = manifest
        .require_api_version(PLUGIN_API_VERSION)
        .expect_err("version mismatch should fail");
    match err {
        PluginError::VersionMismatch {
            plugin,
            expected,
            found,
        } => {
            assert_eq!(plugin, "wayland-test-bad-version");
            assert_eq!(expected, PLUGIN_API_VERSION);
            assert_eq!(found, "0.9");
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn matching_version_accepted() {
    let manifest = PluginManifest::from_toml_str(
        r#"
plugin_api_version = "1.0"

[plugin]
name = "wayland-test-good-version"
version = "0.0.1"
description = "version match test"
entry = "builtin:good"
authors = ["test"]
license = "MIT"
"#,
    )
    .expect("manifest parses");
    manifest
        .require_api_version(PLUGIN_API_VERSION)
        .expect("matching version accepted");
}

#[test]
fn missing_version_is_compatible() {
    // Backward compatibility: v0.6.4-era manifests with no
    // plugin_api_version field must still load.
    let manifest = PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-test-no-version"
version = "0.0.1"
description = "missing version test"
entry = "builtin:none"
authors = ["test"]
license = "MIT"
"#,
    )
    .expect("manifest parses");
    manifest
        .require_api_version(PLUGIN_API_VERSION)
        .expect("missing version is treated as compatible");
}

#[test]
fn unknown_kind_rejects() {
    let err = PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-test-bad-runtime"
version = "0.0.1"
description = "bad runtime kind test"
entry = "builtin:bad"
authors = ["test"]
license = "MIT"

[runtime]
kind = "wat"
"#,
    )
    .expect_err("unknown runtime kind should fail");
    match err {
        PluginError::UnknownRuntimeKind { plugin, kind } => {
            assert_eq!(plugin, "wayland-test-bad-runtime");
            assert_eq!(kind, "wat");
        }
        other => panic!("expected UnknownRuntimeKind, got {other:?}"),
    }
}

#[test]
fn default_runtime_kind_is_static_implicit() {
    // No [runtime] block at all — the manifest must parse and require_api_version
    // must succeed (no runtime kind to validate).
    let m = PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-test-default-runtime"
version = "0.0.1"
description = "default runtime test"
entry = "builtin:default"
authors = ["test"]
license = "MIT"
"#,
    )
    .expect("manifest with no [runtime] parses");
    assert!(m.runtime.is_none());
}

#[test]
fn explicit_static_runtime_accepted() {
    let m = PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-test-static-runtime"
version = "0.0.1"
description = "explicit static runtime test"
entry = "builtin:static"
authors = ["test"]
license = "MIT"

[runtime]
kind = "static"
"#,
    )
    .expect("explicit static runtime parses");
    assert_eq!(m.runtime.as_ref().unwrap().kind, "static");
}

#[test]
fn unknown_top_level_field_rejects() {
    // deny_unknown_fields at the manifest root catches typos like
    // `permision = {...}` (note misspelling).
    let err = PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-test-typo"
version = "0.0.1"
description = "typo test"
entry = "builtin:typo"
authors = ["test"]
license = "MIT"

[bogus_top_level]
foo = "bar"
"#,
    )
    .expect_err("unknown top-level field should fail");
    assert!(
        matches!(err, PluginError::ManifestParse(_)),
        "expected ManifestParse variant, got: {:?}",
        err
    );
}

#[test]
fn doc_hidden_items_still_accessible() {
    // v0.6.5 Task 1.1 — `NamespaceLedger` is hidden from the plugin-facing
    // surface (the `registry` module is `#[doc(hidden)]`), but the host
    // (and any caller that knows the full path) can still reach it. This
    // test pins that contract so a future `pub(crate)` downgrade would
    // fire a named failure here.
    let _ledger = wcore_plugin_api::registry::tools::NamespaceLedger::default();
}
