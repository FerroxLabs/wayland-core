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

pub mod catalog;
pub mod error;
pub mod index;
pub mod install;
pub mod known;
pub mod lockfile;
pub mod manifest;
pub mod marketplace;
pub mod quarantine;
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
    /// Install a plugin. `name@marketplace` installs from a registered
    /// Claude Code marketplace (see `plugin marketplace add`); a bare `name`
    /// uses the legacy registry / GitHub-Releases path.
    Install {
        /// `<plugin>@<marketplace>` for a marketplace install, or a bare
        /// kebab-case `name` for the legacy registry path.
        name: String,
        /// Source spec for the legacy path. `local` reads from
        /// `--registry-dir` or the embedded default registry.
        /// `github://<org>` resolves against GitHub Releases. Ignored for
        /// `name@marketplace` installs.
        #[arg(long, default_value = "local")]
        source: String,
        /// Override the local registry directory (legacy path only).
        #[arg(long)]
        registry_dir: Option<PathBuf>,
        /// Print the install plan (consent surface) and exit without writing
        /// anything. Only meaningful for `name@marketplace` installs.
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage Claude Code plugin marketplaces.
    Marketplace {
        #[command(subcommand)]
        cmd: MarketplaceCmd,
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

/// `plugin marketplace <cmd>` — register and inspect Claude Code marketplaces.
#[derive(Debug, Subcommand)]
pub enum MarketplaceCmd {
    /// Register a marketplace: `owner/repo`, a git URL, or a local path to a
    /// dir containing `.claude-plugin/marketplace.json`.
    Add {
        /// The marketplace source.
        source: String,
    },
    /// List registered marketplaces.
    List {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Remove a registered marketplace by its name.
    Remove {
        /// Marketplace name (its declared `name`, not the source).
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
    // Marketplace plugins install into a discovery root the on-disk loader
    // scans (`~/.wayland/plugins`), distinct from the legacy registry root
    // above. `--install-root` overrides both (used by tests).
    let marketplace_root = match &args.install_root {
        Some(p) => p.clone(),
        None => wcore_config::config::profile_home().join("plugins"),
    };
    match args.cmd {
        PluginCmd::Install {
            name,
            source,
            registry_dir,
            dry_run,
        } => {
            if let Some((plugin, market)) = name.split_once('@') {
                let quarantine_root = marketplace_root.join(".quarantine");
                let planned = marketplace::resolve_and_plan(
                    &marketplace_root,
                    &quarantine_root,
                    market,
                    plugin,
                )?;
                println!("{}", planned.plan.render());
                if dry_run {
                    println!("(dry run — nothing installed)");
                } else {
                    let installed_at =
                        humantime::format_rfc3339(std::time::SystemTime::now()).to_string();
                    let dir =
                        marketplace::commit_install(&marketplace_root, &planned, installed_at)?;
                    println!("installed {plugin}@{market} → {}", dir.display());
                }
                return Ok(());
            }
            if dry_run {
                anyhow::bail!(
                    "--dry-run is only supported for marketplace installs (name@marketplace)"
                );
            }
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
            let legacy = install::list_installed(&install_root)?;
            let market = marketplace::list_marketplace_installed(&marketplace_root)?;
            if legacy.is_empty() && market.is_empty() {
                println!("(no plugins installed)");
            }
            for mf in legacy {
                println!("{}\t{}\t{}", mf.name, mf.version, mf.description);
            }
            for p in market {
                println!("{}@{}\t{}\t{}", p.plugin, p.marketplace, p.version, p.grade);
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
        PluginCmd::Marketplace { cmd } => match cmd {
            MarketplaceCmd::Add { source } => {
                let quarantine_root = marketplace_root.join(".quarantine");
                let meta = marketplace::add_marketplace_source(
                    &marketplace_root,
                    &quarantine_root,
                    &source,
                )?;
                println!("added marketplace '{}'", meta.name);
            }
            MarketplaceCmd::List { json } => {
                let list = known::list_marketplaces(&marketplace_root)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&list)?);
                } else if list.is_empty() {
                    println!("(no marketplaces registered)");
                } else {
                    for m in list {
                        let tag = if m.official { " (official)" } else { "" };
                        println!("{}\t{}{}", m.name, m.source, tag);
                    }
                }
            }
            MarketplaceCmd::Remove { name } => {
                if known::remove_marketplace(&marketplace_root, &name)? {
                    catalog::remove_catalog(&marketplace_root, &name)?;
                    println!("removed marketplace '{name}'");
                } else {
                    println!("no such marketplace '{name}'");
                }
            }
        },
    }
    Ok(())
}
