// M5.4: name-validation + URL-construction tests. No network. Locks
// down the M5.8 threat-model invariants from day one — every change to
// `validate_plugin_name` or `release_api_url` MUST keep these green.

#![allow(clippy::panic, clippy::unwrap_used)]

#[test]
fn validates_plugin_name_rejects_path_traversal() {
    use wcore_cli::plugin::resolver::validate_plugin_name;
    assert!(validate_plugin_name("wayland-honcho").is_ok());
    assert!(validate_plugin_name("ollama").is_ok());
    assert!(validate_plugin_name("a").is_ok());
    assert!(validate_plugin_name("a-b-c-1").is_ok());

    // Path traversal / separators.
    assert!(validate_plugin_name("..").is_err());
    assert!(validate_plugin_name("../foo").is_err());
    assert!(validate_plugin_name("foo/bar").is_err());
    assert!(validate_plugin_name("foo\\bar").is_err());
    assert!(validate_plugin_name("foo/../bar").is_err());

    // Bad first character.
    assert!(validate_plugin_name("Foo").is_err());
    assert!(validate_plugin_name("123-leading-digit").is_err());
    assert!(validate_plugin_name("-leading-dash").is_err());

    // Bad chars anywhere.
    assert!(validate_plugin_name("foo_bar").is_err());
    assert!(validate_plugin_name("foo bar").is_err());
    assert!(validate_plugin_name("foo!").is_err());

    // Empty.
    assert!(validate_plugin_name("").is_err());
}

#[test]
fn github_release_url_is_constructed_safely() {
    use wcore_cli::plugin::resolver::GitHubReleasesResolver;
    let r = GitHubReleasesResolver::new("FerroxLabs");
    let url = r.release_api_url("wayland-honcho").unwrap();
    assert_eq!(
        url.as_str(),
        "https://api.github.com/repos/FerroxLabs/wayland-honcho/releases/latest"
    );

    // Malicious names rejected before any URL is built.
    assert!(r.release_api_url("..").is_err());
    assert!(r.release_api_url("foo/../bar").is_err());
    assert!(r.release_api_url("foo\\bar").is_err());
    assert!(r.release_api_url("").is_err());
    assert!(r.release_api_url("Foo").is_err());
}

#[test]
fn local_file_resolver_round_trips_a_manifest() {
    use wcore_cli::plugin::resolver::{LocalFileResolver, Resolver};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    std::fs::write(
        dir.join("wayland-honcho.toml"),
        r#"
name = "wayland-honcho"
version = "0.6.0"
description = "honcho shell"
"#,
    )
    .unwrap();
    let r = LocalFileResolver::new(&dir);
    let mf = r.resolve_manifest("wayland-honcho").unwrap();
    assert_eq!(mf.name, "wayland-honcho");
    assert_eq!(mf.version, "0.6.0");
    assert_eq!(mf.description, "honcho shell");
    assert!(!mf.requires_sandbox);

    // Unknown plugin → NotInRegistry (not InvalidName — name is valid).
    let err = r.resolve_manifest("wayland-wormhole").unwrap_err();
    assert!(matches!(
        err,
        wcore_cli::plugin::error::PluginCliError::NotInRegistry(_)
    ));

    // Invalid name → InvalidName before any filesystem access.
    let err2 = r.resolve_manifest("..").unwrap_err();
    assert!(matches!(
        err2,
        wcore_cli::plugin::error::PluginCliError::InvalidName(_)
    ));
}

#[test]
fn install_via_local_file_resolver_writes_record() {
    use wcore_cli::plugin::install::{install_via_resolver, list_installed};
    use wcore_cli::plugin::resolver::LocalFileResolver;
    let tmp = tempfile::tempdir().unwrap();
    let registry = tmp.path().join("reg");
    let install_root = tmp.path().join("inst");
    std::fs::create_dir_all(&registry).unwrap();
    std::fs::write(
        registry.join("wayland-ollama.toml"),
        r#"
name = "wayland-ollama"
version = "0.6.0"
"#,
    )
    .unwrap();
    let r = LocalFileResolver::new(&registry);
    install_via_resolver(&r, "wayland-ollama", &install_root).unwrap();
    let entries = list_installed(&install_root).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "wayland-ollama");
}
