//! Integration tests for `wayland-core migrate hermes` (issue #228).
//!
//! Drives the public importer against a fixture Hermes home with `WAYLAND_HOME`
//! pointed at a tempdir, then reads back the written `config.toml`. Serialized
//! because the importer resolves `config.toml` through the process-global
//! `WAYLAND_HOME` env var (same discipline as the `profile` CLI tests — avoids
//! the env-race class that #192/#196 fenced).

use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;
use wcore_cli::migrate::{self, HermesArgs, MigrateCmd};

/// RAII env guard restoring prior values on drop.
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl EnvGuard {
    fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(k, v)| {
                let prev = std::env::var_os(k);
                match v {
                    Some(val) => unsafe { std::env::set_var(k, val) },
                    None => unsafe { std::env::remove_var(k) },
                }
                (*k, prev)
            })
            .collect();
        Self(saved)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.0 {
            match prev {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

/// A tempdir `WAYLAND_HOME` (so `config.toml` resolves there) with `XDG_DATA_HOME`
/// cleared so it can't shadow the override on Linux.
fn rooted() -> (EnvGuard, TempDir) {
    let home = tempfile::tempdir().unwrap();
    let g = EnvGuard::set(&[
        ("WAYLAND_HOME", Some(home.path().to_str().unwrap())),
        ("XDG_DATA_HOME", None),
    ]);
    (g, home)
}

/// Build a fixture Hermes home with two profiles: `alpha` (full: model + mcp +
/// key + persona + skill) and `beta` (bare model only).
fn fixture_hermes() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let alpha = dir.path().join("profiles/alpha");
    std::fs::create_dir_all(alpha.join("skills/foo")).unwrap();
    std::fs::create_dir_all(alpha.join("memories")).unwrap();
    std::fs::write(
        alpha.join("config.yaml"),
        "model:\n  default: deepseek/deepseek-v4-pro\n  provider: deepseek\n  base_url: https://api.deepseek.com/v1\n  api_mode: chat_completions\nmcp_servers:\n  ijfw-memory:\n    command: /usr/bin/ijfw-memory\n    args: []\n    env:\n      PATH: /usr/bin\nterminal:\n  ignored: true\n",
    )
    .unwrap();
    std::fs::write(alpha.join(".env"), "DEEPSEEK_API_KEY=sk-secret-alpha\n").unwrap();
    std::fs::write(alpha.join("SOUL.md"), "You are alpha.").unwrap();
    std::fs::write(
        alpha.join("skills/foo/SKILL.md"),
        "---\nname: foo\n---\nbody",
    )
    .unwrap();
    std::fs::write(alpha.join("memories/note.md"), "a memory").unwrap();
    std::fs::write(alpha.join("memories/MEMORY.md"), "entrypoint").unwrap();

    let beta = dir.path().join("profiles/beta");
    std::fs::create_dir_all(&beta).unwrap();
    std::fs::write(
        beta.join("config.yaml"),
        "model:\n  default: claude-opus\n  provider: anthropic\n",
    )
    .unwrap();
    dir
}

fn config_toml(home: &Path) -> String {
    std::fs::read_to_string(home.join("config.toml")).unwrap_or_default()
}

fn hermes_args(home: &Path, include_credentials: bool, overwrite: bool) -> HermesArgs {
    HermesArgs {
        home: Some(home.to_path_buf()),
        dry_run: false,
        yes: true,
        include_credentials,
        overwrite,
    }
}

#[test]
#[serial]
fn build_plan_maps_hermes_profiles() {
    let _g = rooted().0; // isolate global_profiles() reads
    let hermes = fixture_hermes();
    let plan = migrate::hermes::build_plan(hermes.path(), false).unwrap();

    assert_eq!(plan.source, "hermes");
    assert_eq!(plan.profiles.len(), 2, "alpha + beta");

    // Sorted: alpha first.
    let alpha = &plan.profiles[0];
    assert_eq!(alpha.name, "alpha");
    assert_eq!(alpha.config.provider.as_deref(), Some("deepseek"));
    // "deepseek/" prefix stripped against the matching provider.
    assert_eq!(alpha.config.model.as_deref(), Some("deepseek-v4-pro"));
    assert_eq!(
        alpha.config.base_url.as_deref(),
        Some("https://api.deepseek.com/v1")
    );
    assert_eq!(alpha.mcp_refs, vec!["ijfw-memory".to_string()]);
    assert!(alpha.has_credential);
    assert_eq!(
        alpha.credential_env_var.as_deref(),
        Some("DEEPSEEK_API_KEY")
    );
    // include_credentials=false ⇒ the value is NOT loaded into the plan.
    assert!(alpha.config.api_key.is_none());

    // New MCP server captured globally.
    assert!(plan.mcp_servers.contains_key("ijfw-memory"));
    let srv = &plan.mcp_servers["ijfw-memory"];
    assert_eq!(srv.command.as_deref(), Some("/usr/bin/ijfw-memory"));

    // Deferred inventory counted, not imported.
    assert_eq!(plan.deferred.skills, 1);
    assert_eq!(plan.deferred.personas, 1);
    assert_eq!(plan.deferred.memory_files, 1, "MEMORY.md excluded");

    let beta = &plan.profiles[1];
    assert_eq!(beta.name, "beta");
    assert_eq!(beta.config.provider.as_deref(), Some("anthropic"));
    assert!(!beta.has_credential);
}

#[test]
#[serial]
fn apply_writes_profiles_and_mcp_without_secrets_by_default() {
    let (_g, home) = rooted();
    let hermes = fixture_hermes();

    migrate::run(MigrateCmd::Hermes(hermes_args(hermes.path(), false, false))).unwrap();

    let names: Vec<String> = wcore_config::config::global_profiles()
        .into_iter()
        .map(|(n, _, _)| n)
        .collect();
    assert!(names.contains(&"alpha".to_string()));
    assert!(names.contains(&"beta".to_string()));

    let toml = config_toml(home.path());
    assert!(toml.contains("[profiles.alpha]"));
    assert!(toml.contains("deepseek-v4-pro"));
    assert!(toml.contains("[mcp.servers.ijfw-memory]"));
    // No credentials imported by default.
    assert!(
        !toml.contains("sk-secret-alpha"),
        "default import must not write the API key"
    );
    assert!(!toml.contains("api_key"));
}

#[test]
#[serial]
fn include_credentials_writes_key_and_overwrite_replaces() {
    let (_g, home) = rooted();
    let hermes = fixture_hermes();

    // First import without secrets.
    migrate::run(MigrateCmd::Hermes(hermes_args(hermes.path(), false, false))).unwrap();
    assert!(!config_toml(home.path()).contains("sk-secret-alpha"));

    // Re-run WITH credentials + overwrite ⇒ key now present.
    migrate::run(MigrateCmd::Hermes(hermes_args(hermes.path(), true, true))).unwrap();
    let toml = config_toml(home.path());
    assert!(
        toml.contains("sk-secret-alpha"),
        "--include-credentials must write the API key"
    );
}

#[test]
#[serial]
fn overwrite_without_credentials_preserves_secret_and_manual_fields() {
    let (_g, home) = rooted();
    let hermes = fixture_hermes();

    // Import WITH credentials → the api_key is stored.
    migrate::run(MigrateCmd::Hermes(hermes_args(hermes.path(), true, false))).unwrap();
    assert!(config_toml(home.path()).contains("sk-secret-alpha"));

    // Hand-add an importer-UNmanaged field to the imported profile.
    wcore_config::config::patch_global_config(|f| {
        if let Some(p) = f.profiles.get_mut("alpha") {
            p.max_tokens = Some(4096);
        }
    })
    .unwrap();

    // Re-sync WITHOUT --include-credentials but WITH --overwrite. This must
    // refresh the managed fields WITHOUT wiping the stored secret or the
    // hand-added field (the #228 cross-audit BLOCKER).
    migrate::run(MigrateCmd::Hermes(hermes_args(hermes.path(), false, true))).unwrap();

    let toml = config_toml(home.path());
    assert!(
        toml.contains("sk-secret-alpha"),
        "overwrite without --include-credentials must preserve the stored api_key"
    );
    assert!(
        toml.contains("4096"),
        "hand-added max_tokens must survive an overwrite"
    );
    // Importer-managed field still present.
    assert!(toml.contains("deepseek-v4-pro"));
}

#[test]
#[serial]
fn import_is_idempotent_without_overwrite() {
    let (_g, home) = rooted();
    let hermes = fixture_hermes();

    migrate::run(MigrateCmd::Hermes(hermes_args(hermes.path(), false, false))).unwrap();
    let first = config_toml(home.path());

    // Second run without --overwrite is a no-op (existing profiles skipped) and
    // must not error or duplicate entries.
    migrate::run(MigrateCmd::Hermes(hermes_args(hermes.path(), false, false))).unwrap();
    let second = config_toml(home.path());

    assert_eq!(
        first, second,
        "re-import without --overwrite must not change config"
    );
    // Exactly one alpha table.
    assert_eq!(second.matches("[profiles.alpha]").count(), 1);
}

#[test]
#[serial]
fn missing_hermes_home_errors() {
    let _g = rooted().0;
    let missing: PathBuf = tempfile::tempdir().unwrap().path().join("nope");
    let err = migrate::hermes::detect_home(Some(&missing)).unwrap_err();
    assert!(err.to_string().contains("no Hermes profiles"));
}
