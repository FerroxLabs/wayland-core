//! CLI surface: `wayland-core profile` — manage isolated profiles.
//!
//! Each profile is a self-contained `WAYLAND_HOME`-rooted home (its own config,
//! credentials, OAuth, memory, skills) stored under `$WAYLAND_PROFILES_ROOT`
//! (default `<os-config>/wayland-core-profiles/`). All active-pointer writes and
//! profile enumeration live in [`wcore_config::profile`] — the D2 single-reader
//! lint (`wcore-config/tests/active_pointer_single_reader_test.rs`) requires every
//! pointer read/write to stay in that one module, so this CLI layer NEVER touches
//! the pointer file directly; it calls the sanctioned helpers
//! ([`wcore_config::profile::set_active_profile`],
//! [`wcore_config::profile::active_profile_name`], …).
//!
//! `run` is the production entry; tests drive it directly under an `EnvGuard`
//! that points `WAYLAND_PROFILES_ROOT` at a tempdir (the same pattern the
//! `wcore_config::profile` unit tests use), so every verb is exercised against
//! an isolated root without touching the real user home.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use wcore_config::profile;

/// Isolated-profile management subcommands.
#[derive(Subcommand, Debug)]
pub enum ProfileCmd {
    /// Create a new isolated profile.
    ///
    /// The profile starts empty (its own config/credentials/memory). With
    /// `--base <name>` an inheritance marker is recorded (resolved at launch in a
    /// later release; NO state — and never any secrets — is copied at create
    /// time). Pass `--use` to also activate it for future launches.
    Create {
        /// Name of the new profile (ASCII letters/digits/`.`/`_`/`-`, case-folded).
        name: String,
        /// Record an inheritance base (an existing profile). No state is copied.
        #[arg(long, value_name = "NAME")]
        base: Option<String>,
        /// Also set the new profile active for future launches.
        #[arg(long = "use")]
        activate: bool,
    },
    /// Set the active profile for future launches.
    ///
    /// Writes the `active` pointer; subsequent `wayland-core` invocations that do
    /// not pass `--profile` or set `WAYLAND_HOME` will use it.
    Use {
        /// Name of an existing profile to activate.
        name: String,
    },
    /// List all profiles (the active one is marked with `*`).
    List,
    /// Show a profile's home path and which stores it contains.
    ///
    /// Defaults to the active profile. Never prints secret values — only whether
    /// each store is present.
    Show {
        /// Profile to inspect (defaults to the active profile).
        name: Option<String>,
    },
    /// Rename a profile. Re-points the active selection if it named `old`.
    Rename {
        /// Current profile name.
        old: String,
        /// New profile name (must not already exist).
        new: String,
    },
    /// Delete a profile and its entire home directory.
    ///
    /// Requires `--yes` (or an interactive confirm on a TTY). Refuses to delete
    /// the currently-active profile without `--force`.
    Delete {
        /// Profile name to delete.
        name: String,
        /// Skip the interactive confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Allow deleting the currently-active profile.
        #[arg(long)]
        force: bool,
    },
    /// Export a profile's home tree to a directory.
    ///
    /// Secrets (`credentials*` files and the `oauth/` directory) are EXCLUDED
    /// unless `--include-secrets` is passed (which prints a stderr warning).
    Export {
        /// Name of the profile to export.
        name: String,
        /// Directory to write the exported tree into (created if absent).
        /// Defaults to `<name>` in the current working directory.
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// Include secrets (`credentials*` and `oauth/`) in the export.
        #[arg(long)]
        include_secrets: bool,
    },
    /// Import a directory tree as a new profile.
    ///
    /// The source must be an existing directory (e.g. a `profile export` tree).
    /// Symlinks are never followed (zip-slip / path-escape defense). The profile
    /// name defaults to the source directory's file name.
    Import {
        /// Path to the directory to adopt as the new profile.
        path: PathBuf,
        /// Name for the imported profile (defaults to the source dir's name).
        #[arg(long = "as", value_name = "NAME")]
        new_name: Option<String>,
    },
}

/// Run a `profile` subcommand. Resolution of the profiles root happens inside the
/// `wcore_config::profile` helpers (via `WAYLAND_PROFILES_ROOT` / the OS config
/// dir), so this entry needs no path argument.
pub fn run(cmd: ProfileCmd) -> Result<()> {
    match cmd {
        ProfileCmd::Create {
            name,
            base,
            activate,
        } => create_cmd(&name, base.as_deref(), activate),
        ProfileCmd::Use { name } => use_cmd(&name),
        ProfileCmd::List => list_cmd(),
        ProfileCmd::Show { name } => show_cmd(name.as_deref()),
        ProfileCmd::Rename { old, new } => rename_cmd(&old, &new),
        ProfileCmd::Delete { name, yes, force } => delete_cmd(&name, yes, force),
        ProfileCmd::Export {
            name,
            out,
            include_secrets,
        } => export_cmd(&name, out.as_deref(), include_secrets),
        ProfileCmd::Import { path, new_name } => import_cmd(&path, new_name.as_deref()),
    }
}

fn create_cmd(name: &str, base: Option<&str>, activate: bool) -> Result<()> {
    let dir = profile::create_profile(name, base)
        .with_context(|| format!("creating profile {name:?}"))?;
    println!("Created profile {name:?} at {}", dir.display());
    if let Some(b) = base {
        println!("  base {b:?} recorded (inherited at launch; no state copied)");
    }
    if activate {
        profile::set_active_profile(name)
            .with_context(|| format!("activating profile {name:?}"))?;
        println!("  set active");
    }
    Ok(())
}

fn use_cmd(name: &str) -> Result<()> {
    profile::set_active_profile(name)
        .with_context(|| format!("setting active profile {name:?}"))?;
    println!("Active profile set to {name:?}.");
    println!(
        "Launches without --profile or WAYLAND_HOME will now use it; run `wayland-core profile show` to confirm."
    );
    Ok(())
}

fn list_cmd() -> Result<()> {
    let profiles = profile::list_profiles();
    if profiles.is_empty() {
        println!("No profiles yet. Create one with `wayland-core profile create <name>`.");
        return Ok(());
    }
    let active = profile::active_profile_name();
    for name in &profiles {
        let marker = if active.as_deref() == Some(name.as_str()) {
            "* "
        } else {
            "  "
        };
        println!("{marker}{name}");
    }
    // A pointer that names a now-deleted profile is reported so the user can fix it.
    if let Some(active) = &active
        && !profiles.contains(active)
    {
        println!();
        println!("warning: active profile {active:?} has no directory (dangling pointer)");
    }
    Ok(())
}

fn show_cmd(name: Option<&str>) -> Result<()> {
    let name = match name {
        Some(n) => n.to_string(),
        None => profile::active_profile_name()
            .context("no profile specified and no active profile set")?,
    };
    if !profile::profile_exists(&name) {
        bail!("profile {name:?} does not exist");
    }
    let dir = profile::profile_dir(&name)?;
    let is_active =
        profile::active_profile_name().as_deref() == Some(name.to_ascii_lowercase().as_str());
    println!(
        "Profile: {name}{}",
        if is_active { " (active)" } else { "" }
    );
    println!("Home:    {}", dir.display());
    // Store presence only — NEVER print secret values.
    let credentials_present = [
        "credentials.toml",
        "credentials.enc",
        "credentials.kdf.json",
    ]
    .iter()
    .any(|f| dir.join(f).exists());
    let rows = [
        ("config", dir.join("config.toml").exists()),
        ("credentials", credentials_present),
        ("oauth", dir.join("oauth").is_dir()),
        ("memory", dir.join("memory").is_dir()),
        ("skills", dir.join("skills").is_dir()),
    ];
    for (label, present) in rows {
        println!("  {label:<12} {}", if present { "present" } else { "—" });
    }
    Ok(())
}

fn rename_cmd(old: &str, new: &str) -> Result<()> {
    profile::rename_profile(old, new)
        .with_context(|| format!("renaming profile {old:?} -> {new:?}"))?;
    println!("Renamed profile {old:?} -> {new:?}.");
    Ok(())
}

fn delete_cmd(name: &str, yes: bool, force: bool) -> Result<()> {
    if !profile::profile_exists(name) {
        bail!("profile {name:?} does not exist");
    }
    // Refuse the active profile unless explicitly forced.
    if profile::is_active(name) && !force {
        bail!("{name:?} is the active profile — pass --force to delete it anyway");
    }
    // Confirm unless --yes; on a non-interactive stdin, refuse rather than block.
    if !yes {
        if std::io::stdin().is_terminal() {
            print!("Delete profile {name:?} and its entire home directory? [y/N] ");
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .context("reading confirmation")?;
            if !matches!(line.trim(), "y" | "Y" | "yes" | "Yes") {
                println!("Aborted.");
                return Ok(());
            }
        } else {
            bail!("refusing to delete {name:?} without --yes (no interactive terminal)");
        }
    }
    profile::delete_profile(name).with_context(|| format!("deleting profile {name:?}"))?;
    println!("Deleted profile {name:?}.");
    Ok(())
}

fn export_cmd(name: &str, out: Option<&Path>, include_secrets: bool) -> Result<()> {
    if !profile::profile_exists(name) {
        bail!("profile {name:?} does not exist");
    }
    let dst = match out {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(name.to_ascii_lowercase()),
    };
    if include_secrets {
        eprintln!(
            "warning: exporting SECRETS (credentials + oauth) for {name:?} into {} — keep this export private",
            dst.display()
        );
    }
    let written = profile::export_profile(name, &dst, include_secrets)
        .with_context(|| format!("exporting profile {name:?}"))?;
    println!(
        "Exported profile {name:?} to {} ({}).",
        written.display(),
        if include_secrets {
            "including secrets"
        } else {
            "secrets excluded"
        }
    );
    Ok(())
}

fn import_cmd(path: &Path, new_name: Option<&str>) -> Result<()> {
    let name = match new_name {
        Some(n) => n.to_string(),
        None => path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .context("could not derive a profile name from the source path; pass --as <name>")?,
    };
    let dir = profile::import_profile(&name, path)
        .with_context(|| format!("importing profile {name:?} from {}", path.display()))?;
    println!(
        "Imported profile {name:?} from {} into {}.",
        path.display(),
        dir.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::tempdir;

    /// RAII env guard — restores prior values on drop so env-mutating tests stay
    /// hermetic. Mirrors the guard in `wcore_config::profile`'s unit tests.
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

    /// Point `WAYLAND_PROFILES_ROOT` at a fresh tempdir; all CLI tests run serial
    /// because they mutate the process env.
    fn rooted() -> (EnvGuard, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let g = EnvGuard::set(&[
            ("WAYLAND_PROFILES_ROOT", Some(dir.path().to_str().unwrap())),
            ("WAYLAND_HOME", None),
        ]);
        (g, dir)
    }

    fn create(name: &str, activate: bool) -> Result<()> {
        run(ProfileCmd::Create {
            name: name.to_string(),
            base: None,
            activate,
        })
    }

    #[test]
    #[serial]
    fn create_makes_dir_and_lists_without_activating() {
        let (_g, _root) = rooted();
        create("work", false).unwrap();
        assert!(profile::profile_exists("work"));
        assert_eq!(profile::list_profiles(), vec!["work"]);
        // No --use → not active.
        assert_eq!(profile::active_profile_name(), None);
    }

    #[test]
    #[serial]
    fn create_with_use_sets_active() {
        let (_g, _root) = rooted();
        create("work", true).unwrap();
        assert_eq!(profile::active_profile_name(), Some("work".to_string()));
    }

    #[test]
    #[serial]
    fn create_with_base_records_marker_not_secrets() {
        let (_g, _root) = rooted();
        let base = profile::create_profile("base", None).unwrap();
        std::fs::write(base.join("credentials.toml"), "secret").unwrap();
        run(ProfileCmd::Create {
            name: "child".to_string(),
            base: Some("base".to_string()),
            activate: false,
        })
        .unwrap();
        let child = profile::profile_dir("child").unwrap();
        assert!(child.join("profile.toml").exists());
        assert!(
            !child.join("credentials.toml").exists(),
            "secret not copied"
        );
        // A missing base is a clear error, not a half-made profile.
        assert!(
            run(ProfileCmd::Create {
                name: "orphan".to_string(),
                base: Some("ghost".to_string()),
                activate: false,
            })
            .is_err()
        );
        assert!(!profile::profile_exists("orphan"));
    }

    #[test]
    #[serial]
    fn use_sets_active_and_errors_on_missing() {
        let (_g, _root) = rooted();
        create("work", false).unwrap();
        run(ProfileCmd::Use {
            name: "Work".to_string(), // case-folds
        })
        .unwrap();
        assert_eq!(profile::active_profile_name(), Some("work".to_string()));
        assert!(
            run(ProfileCmd::Use {
                name: "ghost".to_string()
            })
            .is_err()
        );
    }

    #[test]
    #[serial]
    fn list_marks_active_and_reports_empty() {
        let (_g, _root) = rooted();
        // Empty state is not an error.
        list_cmd().unwrap();
        create("a", false).unwrap();
        create("b", true).unwrap();
        assert_eq!(profile::list_profiles(), vec!["a", "b"]);
        list_cmd().unwrap();
    }

    #[test]
    #[serial]
    fn rename_moves_and_repoints_active() {
        let (_g, _root) = rooted();
        create("old", true).unwrap();
        run(ProfileCmd::Rename {
            old: "old".to_string(),
            new: "new".to_string(),
        })
        .unwrap();
        assert!(!profile::profile_exists("old"));
        assert!(profile::profile_exists("new"));
        assert_eq!(profile::active_profile_name(), Some("new".to_string()));
        // Renaming onto an existing name errors.
        create("taken", false).unwrap();
        assert!(
            run(ProfileCmd::Rename {
                old: "new".to_string(),
                new: "taken".to_string()
            })
            .is_err()
        );
    }

    #[test]
    #[serial]
    fn delete_refuses_active_without_force() {
        let (_g, _root) = rooted();
        create("work", true).unwrap();
        let err = run(ProfileCmd::Delete {
            name: "work".to_string(),
            yes: true,
            force: false,
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("active"));
        assert!(profile::profile_exists("work"), "must survive the refusal");
    }

    #[test]
    #[serial]
    fn delete_without_yes_on_non_tty_is_refused() {
        let (_g, _root) = rooted();
        create("work", false).unwrap();
        // Precondition: not active, so we exercise the TTY/--yes gate, not the
        // active-refuse gate. Under `cargo test` stdin is not a terminal.
        assert!(!profile::is_active("work"));
        assert!(
            run(ProfileCmd::Delete {
                name: "work".to_string(),
                yes: false,
                force: false,
            })
            .is_err()
        );
        assert!(profile::profile_exists("work"));
    }

    #[test]
    #[serial]
    fn delete_with_yes_removes_and_force_clears_active_pointer() {
        let (_g, _root) = rooted();
        create("work", false).unwrap();
        run(ProfileCmd::Delete {
            name: "work".to_string(),
            yes: true,
            force: false,
        })
        .unwrap();
        assert!(!profile::profile_exists("work"));

        create("active-one", true).unwrap();
        run(ProfileCmd::Delete {
            name: "active-one".to_string(),
            yes: true,
            force: true,
        })
        .unwrap();
        assert!(!profile::profile_exists("active-one"));
        assert_eq!(
            profile::active_profile_name(),
            None,
            "deleting the active profile clears the pointer"
        );
    }

    #[test]
    #[serial]
    fn export_excludes_secrets_by_default() {
        let (_g, _root) = rooted();
        let dir = profile::create_profile("work", None).unwrap();
        std::fs::write(dir.join("config.toml"), "model='x'").unwrap();
        std::fs::write(dir.join("credentials.toml"), "secret").unwrap();
        std::fs::create_dir_all(dir.join("oauth")).unwrap();
        std::fs::write(dir.join("oauth/t.json"), "{}").unwrap();

        let out = tempdir().unwrap();
        let dst = out.path().join("exp");
        run(ProfileCmd::Export {
            name: "work".to_string(),
            out: Some(dst.clone()),
            include_secrets: false,
        })
        .unwrap();
        assert!(dst.join("config.toml").exists());
        assert!(!dst.join("credentials.toml").exists(), "secret excluded");
        assert!(!dst.join("oauth").exists(), "oauth excluded");

        // --include-secrets copies them.
        let dst2 = out.path().join("exp2");
        run(ProfileCmd::Export {
            name: "work".to_string(),
            out: Some(dst2.clone()),
            include_secrets: true,
        })
        .unwrap();
        assert!(dst2.join("credentials.toml").exists());
    }

    #[test]
    #[serial]
    fn import_creates_profile_and_derives_name_from_dir() {
        let (_g, _root) = rooted();
        let src = tempdir().unwrap();
        let tree = src.path().join("acme");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("config.toml"), "model='y'").unwrap();

        run(ProfileCmd::Import {
            path: tree.clone(),
            new_name: None,
        })
        .unwrap();
        assert!(profile::profile_exists("acme"));
        // Re-importing onto the same name errors.
        assert!(
            run(ProfileCmd::Import {
                path: tree,
                new_name: Some("acme".to_string()),
            })
            .is_err()
        );
    }

    #[test]
    #[serial]
    fn show_runs_for_active_and_named() {
        let (_g, _root) = rooted();
        create("work", true).unwrap();
        // Default = active.
        run(ProfileCmd::Show { name: None }).unwrap();
        // Explicit name.
        run(ProfileCmd::Show {
            name: Some("work".to_string()),
        })
        .unwrap();
        // Unknown name errors.
        assert!(
            run(ProfileCmd::Show {
                name: Some("ghost".to_string())
            })
            .is_err()
        );
    }
}
