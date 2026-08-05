use std::ffi::{OsStr, OsString};
use std::path::Path;

use serial_test::serial;
use wcore_config::config::{CliArgs, Config, effective_config_toml_with_provenance};
use wcore_config::resolution_provenance::{
    ConfigSourceDisposition, ConfigSourceRole, LaunchBindingEvidence,
};

struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn set(values: &[(&'static str, Option<&OsStr>)]) -> Self {
        let saved = values
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in values {
            // SAFETY: every test in this binary that mutates the environment
            // uses the same serial group and restores it through this guard.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            // SAFETY: see `EnvGuard::set`; restoration occurs in the same
            // serialized test before the guard is dropped.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn cli(project_dir: &Path) -> CliArgs {
    CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key-that-must-not-enter-provenance".to_string()),
        project_dir: Some(project_dir.to_path_buf()),
        ..CliArgs::default()
    }
}

fn source_dispositions(
    provenance: &wcore_config::resolution_provenance::ConfigResolutionProvenance,
    role: ConfigSourceRole,
) -> Vec<ConfigSourceDisposition> {
    provenance
        .sources
        .iter()
        .find(|source| source.role == role)
        .unwrap_or_else(|| panic!("missing source role {role:?}"))
        .dispositions
        .clone()
}

#[test]
#[serial(config_provenance_env)]
fn runtime_and_preview_share_sources_and_ignore_legacy_path() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let decoy_root = tempfile::tempdir().unwrap();
    let decoy = decoy_root.path().join("wayland-config.txt");
    std::fs::write(&decoy, "[default]\nprovider = \"openai\"\n").unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[default]\nprovider = \"anthropic\"\nmax_tokens = 1234\n",
    )
    .unwrap();

    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("XDG_DATA_HOME", None),
        ("WAYLAND_CONFIG_PATH", Some(decoy.as_os_str())),
    ]);
    let args = cli(project.path());
    let runtime = Config::resolve_with_provenance(&args).unwrap();
    let preview = effective_config_toml_with_provenance(&args).unwrap();

    assert_eq!(runtime.value.max_tokens, 1234);
    assert_eq!(runtime.provenance, preview.provenance);
    assert_eq!(
        runtime.provenance.launch_binding,
        LaunchBindingEvidence::ExplicitWaylandHome
    );
    let ignored_role = ConfigSourceRole::EnvironmentOverride {
        variable: "WAYLAND_CONFIG_PATH".to_string(),
    };
    assert_eq!(
        source_dispositions(&runtime.provenance, ignored_role),
        vec![ConfigSourceDisposition::Ignored]
    );
    assert!(
        runtime
            .provenance
            .sources
            .iter()
            .all(|source| source.path.as_deref() != Some(decoy.as_path())),
        "the unsupported override value must never become a config source"
    );
    let debug = format!("{:?}", runtime.provenance);
    assert!(!debug.contains("test-key-that-must-not-enter-provenance"));
    assert!(!debug.contains(decoy.to_string_lossy().as_ref()));
}

#[test]
#[serial(config_provenance_env)]
fn unreadable_and_invalid_sources_remain_distinct() {
    let unreadable_home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(unreadable_home.path().join("config.toml")).unwrap();
    let _unreadable_env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(unreadable_home.path().as_os_str())),
        ("XDG_DATA_HOME", None),
        ("WAYLAND_CONFIG_PATH", None),
    ]);
    let resolved = Config::resolve_with_provenance(&cli(project.path())).unwrap();
    assert_eq!(
        source_dispositions(&resolved.provenance, ConfigSourceRole::Global),
        vec![ConfigSourceDisposition::Unreadable]
    );
    drop(_unreadable_env);

    let invalid_home = tempfile::tempdir().unwrap();
    std::fs::write(invalid_home.path().join("config.toml"), "[default\n").unwrap();
    let _invalid_env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(invalid_home.path().as_os_str())),
        ("XDG_DATA_HOME", None),
        ("WAYLAND_CONFIG_PATH", None),
    ]);
    let failure = Config::resolve_with_provenance(&cli(project.path())).unwrap_err();
    assert_eq!(
        source_dispositions(&failure.provenance, ConfigSourceRole::Global),
        vec![ConfigSourceDisposition::Invalid]
    );
}

#[test]
#[serial(config_provenance_env)]
fn untrusted_project_is_recorded_as_loaded_and_restricted() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".wayland-core.toml"),
        "[default]\nmax_tokens = 4321\n",
    )
    .unwrap();
    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("XDG_DATA_HOME", None),
        ("WAYLAND_CONFIG_PATH", None),
    ]);
    let resolved = Config::resolve_with_provenance(&cli(project.path())).unwrap();
    let dispositions = source_dispositions(&resolved.provenance, ConfigSourceRole::Project);
    assert!(dispositions.contains(&ConfigSourceDisposition::Loaded));
    assert!(dispositions.contains(&ConfigSourceDisposition::Restricted));
}

#[test]
#[serial(config_provenance_env)]
fn canonical_project_layout_records_overridden_legacy_layout() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".wayland-core.toml"),
        "[default]\nmax_tokens = 2222\n",
    )
    .unwrap();
    std::fs::create_dir(project.path().join(".wayland-core")).unwrap();
    std::fs::write(
        project.path().join(".wayland-core/config.toml"),
        "[default]\nmax_tokens = 9999\n",
    )
    .unwrap();
    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("XDG_DATA_HOME", None),
        ("WAYLAND_CONFIG_PATH", None),
    ]);
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();
    let resolved = Config::resolve_with_provenance(&CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key".to_string()),
        ..CliArgs::default()
    });
    std::env::set_current_dir(original_dir).unwrap();
    let resolved = resolved.unwrap();

    assert_eq!(resolved.value.max_tokens, 2222);
    let project_sources: Vec<_> = resolved
        .provenance
        .sources
        .iter()
        .filter(|source| source.role == ConfigSourceRole::Project)
        .collect();
    assert_eq!(project_sources.len(), 2);
    assert!(project_sources.iter().any(|source| {
        source
            .dispositions
            .contains(&ConfigSourceDisposition::Overridden)
    }));
}

#[test]
#[serial(config_provenance_env)]
fn missing_profile_failure_retains_invalid_profile_evidence() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().as_os_str())),
        ("XDG_DATA_HOME", None),
        ("WAYLAND_CONFIG_PATH", None),
    ]);
    let mut args = cli(project.path());
    args.profile = Some("missing".to_string());
    let failure = Config::resolve_with_provenance(&args).unwrap_err();
    assert_eq!(
        source_dispositions(&failure.provenance, ConfigSourceRole::Profile),
        vec![ConfigSourceDisposition::Invalid]
    );
}
