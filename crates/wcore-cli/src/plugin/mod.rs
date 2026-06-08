// M5.4: plugin marketplace subcommand.
//
// Wires `wayland-core plugin {install,list,available,remove}` to the
// resolver + registry + install primitives in this module.
//
// Routing:
// - `--source local` (default) reads either a `--registry-dir` of TOML
//   manifests or the embedded `data/registry-default.json`.
// - `--source github://<org>` uses the `GitHubReleasesResolver`. Behind
//   the `remote-registry` feature; default ON for v0.6.
//
// Install root defaults to `dirs::data_dir()/wayland-core/plugins`,
// overridable via `--install-root` (handy for tests + sandbox setups).

pub mod error;
pub mod index;
pub mod install;
pub mod manifest;
pub mod registry;
pub mod resolver;

use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    pub cmd: PluginCmd,

    /// Override the install root. Defaults to
    /// `dirs::data_dir()/wayland-core/plugins`. Mostly useful for tests
    /// and sandboxed setups; users normally don't touch this.
    #[arg(long, global = true)]
    pub install_root: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum PluginCmd {
    /// Install a plugin from the registry or a remote source.
    Install {
        /// Plugin name (kebab-case, e.g. `wayland-honcho`).
        name: String,
        /// Source spec. `local` reads from `--registry-dir` or the
        /// embedded default registry. `github://<org>` resolves
        /// against GitHub Releases on the given org.
        #[arg(long, default_value = "local")]
        source: String,
        /// Override the local registry directory (only honored when
        /// `--source local`). If omitted, the embedded default
        /// registry shipped with the binary is used.
        #[arg(long)]
        registry_dir: Option<PathBuf>,
    },
    /// List installed plugins.
    List,
    /// List plugins available in the local default registry (or the
    /// directory pointed at by `--registry-dir`).
    Available {
        #[arg(long)]
        registry_dir: Option<PathBuf>,
    },
    /// Remove an installed plugin.
    Remove {
        /// Plugin name to remove.
        name: String,
    },
}

/// Synchronous dispatcher — none of the plugin code is async, so we
/// don't need to wrap this in a tokio runtime. The HTTP client in the
/// resolver is `reqwest::blocking`, matching the rest of this module.
pub fn run(args: PluginArgs) -> anyhow::Result<()> {
    let install_root = match &args.install_root {
        Some(p) => p.clone(),
        None => {
            // `dirs::data_dir` returns the platform-appropriate root
            // (XDG_DATA_HOME on Linux, ~/Library/Application Support on
            // macOS, %APPDATA% on Windows). Using it keeps install
            // paths cross-platform.
            let base =
                dirs::data_dir().ok_or_else(|| anyhow::anyhow!("could not determine data_dir"))?;
            base.join("wayland-core").join("plugins")
        }
    };
    match args.cmd {
        PluginCmd::Install {
            name,
            source,
            registry_dir,
        } => {
            if source == "local" {
                let reg = match &registry_dir {
                    Some(dir) => registry::Registry::from_dir(dir)?,
                    None => registry::Registry::load_default()?,
                };
                install::install_from_registry(&reg, &name, &install_root)?;
            } else if let Some(org) = source.strip_prefix("github://") {
                #[cfg(feature = "remote-registry")]
                {
                    let r = resolver::GitHubReleasesResolver::new(org);
                    install::install_via_resolver(&r, &name, &install_root)?;
                }
                #[cfg(not(feature = "remote-registry"))]
                {
                    let _ = org;
                    anyhow::bail!(
                        "remote-registry feature not enabled at build time; \
                         rebuild with --features remote-registry to use github:// sources"
                    );
                }
            } else {
                anyhow::bail!(
                    "unknown plugin source: {source} \
                     (expected 'local' or 'github://<org>')"
                );
            }
            println!("installed {name}");
        }
        PluginCmd::Remove { name } => {
            install::remove(&install_root, &name)?;
            println!("removed {name}");
        }
        PluginCmd::List => {
            let entries = install::list_installed(&install_root)?;
            if entries.is_empty() {
                println!("(no plugins installed)");
            }
            for mf in entries {
                println!("{}\t{}\t{}", mf.name, mf.version, mf.description);
            }
        }
        PluginCmd::Available { registry_dir } => {
            let reg = match registry_dir {
                Some(dir) => registry::Registry::from_dir(&dir)?,
                None => registry::Registry::load_default()?,
            };
            for mf in reg.list_available() {
                println!("{}\t{}\t{}", mf.name, mf.version, mf.description);
            }
        }
    }
    Ok(())
}
