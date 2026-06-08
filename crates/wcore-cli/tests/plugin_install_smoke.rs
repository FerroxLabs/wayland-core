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
    assert!(names.contains(&"wayland-ijfw"));
}
