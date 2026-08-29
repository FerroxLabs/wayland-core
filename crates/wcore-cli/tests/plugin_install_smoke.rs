// M5.4: install → list → remove round-trip against a tempdir registry +
// install root. Pure filesystem; no network.

#![allow(clippy::panic, clippy::unwrap_used)]

#[test]
fn install_then_list_then_remove_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let registry_dir = tmp.path().join("registry");
    let install_root = tmp.path().join("installed");
    std::fs::create_dir_all(&registry_dir).unwrap();

    std::fs::write(
        registry_dir.join("wayland-honcho.toml"),
        r#"
name = "wayland-honcho"
version = "0.6.0"
requires_sandbox = false
description = "Honcho user-model plugin shell"
"#,
    )
    .unwrap();

    let store = wcore_cli::plugin::registry::Registry::from_dir(&registry_dir).unwrap();
    assert_eq!(store.list_available().len(), 1);

    wcore_cli::plugin::install::install_from_registry(&store, "wayland-honcho", &install_root)
        .unwrap();
    let installed = wcore_cli::plugin::install::list_installed(&install_root).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].name, "wayland-honcho");
    assert_eq!(installed[0].version, "0.6.0");

    wcore_cli::plugin::install::remove(&install_root, "wayland-honcho").unwrap();
    let installed_after = wcore_cli::plugin::install::list_installed(&install_root).unwrap();
    assert_eq!(installed_after.len(), 0);
}

#[test]
fn install_unknown_plugin_returns_typed_error() {
    let tmp = tempfile::tempdir().unwrap();
    let registry_dir = tmp.path().join("registry");
    let install_root = tmp.path().join("installed");
    std::fs::create_dir_all(&registry_dir).unwrap();
    let store = wcore_cli::plugin::registry::Registry::from_dir(&registry_dir).unwrap();
    let err = wcore_cli::plugin::install::install_from_registry(
        &store,
        "wayland-wormhole",
        &install_root,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        wcore_cli::plugin::error::PluginCliError::NotInRegistry(_)
    ));
}

#[test]
fn double_install_returns_already_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let registry_dir = tmp.path().join("registry");
    let install_root = tmp.path().join("installed");
    std::fs::create_dir_all(&registry_dir).unwrap();
    std::fs::write(
        registry_dir.join("wayland-honcho.toml"),
        r#"
name = "wayland-honcho"
version = "0.6.0"
"#,
    )
    .unwrap();
    let store = wcore_cli::plugin::registry::Registry::from_dir(&registry_dir).unwrap();
    wcore_cli::plugin::install::install_from_registry(&store, "wayland-honcho", &install_root)
        .unwrap();
    let err =
        wcore_cli::plugin::install::install_from_registry(&store, "wayland-honcho", &install_root)
            .unwrap_err();
    assert!(matches!(
        err,
        wcore_cli::plugin::error::PluginCliError::AlreadyInstalled(_)
    ));
}

#[test]
fn remove_unknown_returns_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let install_root = tmp.path().join("installed");
    std::fs::create_dir_all(&install_root).unwrap();
    let err = wcore_cli::plugin::install::remove(&install_root, "wayland-honcho").unwrap_err();
    assert!(matches!(
        err,
        wcore_cli::plugin::error::PluginCliError::NotInstalled(_)
    ));
}

#[test]
fn list_nonexistent_install_root_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let install_root = tmp.path().join("does-not-exist");
    let entries = wcore_cli::plugin::install::list_installed(&install_root).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn default_registry_loads_all_v0_6_entries() {
    let reg = wcore_cli::plugin::registry::Registry::load_default().unwrap();
    let names: Vec<&str> = reg
        .list_available()
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    // The exact contents are pinned to the v0.6 lineup. Adding a plugin
    // to `registry-default.json` MUST update this assertion so the test
    // keeps the embedded registry honest.
    assert!(names.contains(&"wayland-ollama"));
    assert!(names.contains(&"wayland-honcho"));
    assert!(names.contains(&"wayland-browser"));
    assert!(names.contains(&"wayland-cua"));
    // wayland-ijfw is a declarative-install-only integration (installed via
    // the IJFW skill/MCP path, not bundled into the binary). It must NOT
    // appear in the embedded install catalog.
    assert!(!names.contains(&"wayland-ijfw"));
}

/// Issue #113 — the catalog `wayland plugin list` prints must not advertise a
/// backend that does not exist.
///
/// The embedded description read "Camoufox / chromiumoxide / Browserbase".
/// There is no chromiumoxide backend in this workspace and there never was;
/// `wcore-browser` ships `CamoufoxBackend` and `BrowserbaseBackend` only. This
/// is the user-facing half of the same false claim the report in #113 was
/// built on, so it is guarded on the same terms.
#[test]
fn the_browser_plugin_description_names_only_real_backends() {
    let reg = wcore_cli::plugin::registry::Registry::load_default().unwrap();
    let entries = reg.list_available();
    let browser = entries
        .iter()
        .find(|m| m.name == "wayland-browser")
        .expect("wayland-browser is in the embedded catalog");

    assert!(
        !browser
            .description
            .to_ascii_lowercase()
            .contains("chromiumoxide"),
        "the catalog advertises a chromiumoxide backend that does not exist: {}",
        browser.description
    );
    // POSITIVE CONTROL: the description is still a real description naming the
    // backends that DO exist, not an empty string the assertion above would
    // pass on for the wrong reason.
    assert!(
        browser.description.contains("Camoufox") && browser.description.contains("Browserbase"),
        "the two real backends must still be named: {}",
        browser.description
    );
}
