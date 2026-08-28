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

pub mod approve;
pub mod catalog;
pub mod codex_marketplace;
pub mod error;
pub mod generations;
pub mod index;
pub mod inspect;
pub mod install;
pub mod known;
pub mod lifecycle;
pub mod lockfile;
pub mod manifest;
pub mod marketplace;
pub mod publish;
pub mod quarantine;
pub mod recover;
pub mod registry;
pub mod resolver;
pub mod scaffold;
pub mod sign;
pub mod verify;

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

    // --- F25-04: the governed lifecycle. Author side first, operator side
    // below. Each verb dispatches into its own module so this file stays a
    // dispatcher rather than becoming the implementation.
    /// Scaffold a new plugin from a shipped template.
    New {
        /// Kebab-case plugin name (`^[a-z][a-z0-9-]*$`).
        name: String,
        /// Directory to generate into. The plugin lands in `<path>/<name>`.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Which shipped template: `static` (compiled into the engine) or
        /// `wasm` (an on-disk component plugin).
        #[arg(long, default_value = "static")]
        template: String,
    },
    /// Run a plugin's own test suite and return its verdict faithfully.
    Test {
        /// The plugin's source directory.
        dir: PathBuf,
    },
    /// Report manifest validity, declared permissions and API compatibility.
    /// Non-zero exit on an incompatible declared API version.
    Verify {
        /// The plugin directory to verify.
        dir: PathBuf,
    },
    /// Sign a plugin's entry artifact with the engine's Ed25519 trust root.
    Sign {
        /// The plugin directory to sign.
        dir: Option<PathBuf>,
        /// Path to a raw 32-byte Ed25519 signing key.
        #[arg(long)]
        key: Option<PathBuf>,
        /// Instead of signing, mint a new keypair at this path (writes
        /// `<path>` and `<path>.pub`).
        #[arg(long)]
        new_key: Option<PathBuf>,
    },
    /// Produce a digest-addressed, signed bundle into a local marketplace
    /// directory. Never pushes, merges, releases or deploys.
    Publish {
        /// The signed plugin directory to publish.
        dir: PathBuf,
        /// Target marketplace directory.
        #[arg(long)]
        to: PathBuf,
    },
    /// Report everything known about one installed plugin, including whether
    /// the loader will admit it. Non-zero exit when it is not installed.
    Inspect {
        /// Plugin name, or its `<plugin>@<marketplace>` directory name.
        name: String,
    },
    /// Approve an installed plugin's CURRENT bytes so the loader will run it.
    Approve {
        /// Plugin name, or its `<plugin>@<marketplace>` directory name.
        name: String,
        /// Withdraw approval instead of granting it.
        #[arg(long)]
        revoke: bool,
    },
    /// Re-resolve a plugin from its marketplace, retaining the prior
    /// generation so the update can be rolled back.
    Update {
        /// Plugin name, or its `<plugin>@<marketplace>` directory name.
        name: String,
    },
    /// Restore the exact bytes of the retained prior generation.
    Rollback {
        /// Plugin name, or its `<plugin>@<marketplace>` directory name.
        name: String,
    },
    /// Repair a half-written plugin store (interrupted update, missing live
    /// generation, drifted install directory).
    Recover,
}

/// `plugin marketplace <cmd>` — register and inspect foreign plugin
/// marketplaces (Claude Code and Codex catalogs).
#[derive(Debug, Subcommand)]
pub enum MarketplaceCmd {
    /// Register a marketplace: `owner/repo`, a git URL, or a local path to a
    /// dir containing `.claude-plugin/marketplace.json` or
    /// `.agents/plugins/marketplace.json`.
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
                let installed_at =
                    humantime::format_rfc3339(std::time::SystemTime::now()).to_string();

                // F25-04: acquire FIRST, then branch on what was acquired.
                //
                // The lowering pipeline GENERATES a `plugin.toml` from a
                // canonical draft, which is right for a foreign (Claude Code)
                // plugin and destructive for a Wayland-native one: it would
                // discard the author's manifest, the entry artifact and the
                // detached signature. A signed, digest-addressed bundle has to
                // arrive byte for byte or it is not the thing that was signed.
                // Before this branch existed there was NO install path for a
                // native plugin at all — `plugin install` could only ingest the
                // foreign format.
                let source = marketplace::resolve_source(
                    &marketplace_root,
                    &quarantine_root,
                    market,
                    plugin,
                )?;
                if lifecycle::is_native_bundle(&source.fetched_root) {
                    if dry_run {
                        println!(
                            "would install the Wayland-native plugin {plugin}@{market} \
                             verbatim from {}",
                            source.fetched_root.display()
                        );
                        println!("(dry run — nothing installed)");
                        return Ok(());
                    }
                    let (dir, digest) = lifecycle::native_install(
                        &marketplace_root,
                        market,
                        plugin,
                        &source,
                        installed_at,
                    )?;
                    println!("installed {plugin}@{market} → {}", dir.display());
                    println!("  digest {digest}");
                    println!(
                        "  NOT YET APPROVED — it will be refused at load until you run \
                         `wayland-core plugin approve {plugin}`"
                    );
                    return Ok(());
                }

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
                    let dir = marketplace::commit_install(
                        &marketplace_root,
                        &planned,
                        installed_at.clone(),
                    )?;
                    // Adopt the install into the governed lifecycle: retain it
                    // as a generation and point live at it. Doing this HERE
                    // rather than in a parallel `plugin install --governed` is
                    // what keeps one install path instead of two.
                    let version = lifecycle::manifest_version(&dir);
                    let (_, digest) = lifecycle::adopt_install(
                        &marketplace_root,
                        plugin,
                        &version,
                        &dir,
                        installed_at,
                    )?;
                    println!("installed {plugin}@{market} → {}", dir.display());
                    println!("  digest {digest}");
                    println!(
                        "  NOT YET APPROVED — it will be refused at load until you run \
                         `wayland-core plugin approve {plugin}`"
                    );
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
            // A marketplace install is a `<plugin>@<marketplace>/` directory
            // plus a lockfile record plus (since F25-04) generation and
            // approval state. `install::remove` only ever knew about the legacy
            // `<name>.json` records, so removing a marketplace-installed plugin
            // used to report "not installed" while the directory stayed on
            // disk and kept loading. Try the marketplace path first and fall
            // back to the legacy one.
            let (plugin, market) = match name.split_once('@') {
                Some((p, m)) => (p.to_string(), Some(m.to_string())),
                None => (name.clone(), None),
            };
            let market = match market {
                Some(m) => Some(m),
                None => lockfile::read_lock(&marketplace_root)?
                    .into_iter()
                    .find(|r| r.plugin == plugin)
                    .map(|r| r.marketplace),
            };
            let removed = match &market {
                Some(m) => marketplace::remove_marketplace_plugin(&marketplace_root, &plugin, m)?,
                None => false,
            };
            if removed {
                lifecycle::forget(&marketplace_root, &plugin)?;
                println!("removed {plugin}");
            } else {
                install::remove(&install_root, &name)?;
                println!("removed {name}");
            }
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
        // --- F25-04 lifecycle verbs -------------------------------------
        PluginCmd::New {
            name,
            path,
            template,
        } => {
            scaffold::run_new(&name, &path, scaffold::Template::parse(&template)?)?;
        }
        PluginCmd::Test { dir } => scaffold::run_test(&dir)?,
        PluginCmd::Verify { dir } => verify::run(&dir)?,
        PluginCmd::Sign { dir, key, new_key } => {
            sign::run(dir.as_deref(), key.as_deref(), new_key.as_deref())?
        }
        PluginCmd::Publish { dir, to } => publish::run(
            &dir,
            &to,
            humantime::format_rfc3339(std::time::SystemTime::now()).to_string(),
        )?,
        PluginCmd::Inspect { name } => inspect::run(&marketplace_root, &name)?,
        PluginCmd::Approve { name, revoke } => approve::run(
            &marketplace_root,
            &name,
            revoke,
            humantime::format_rfc3339(std::time::SystemTime::now()).to_string(),
        )?,
        PluginCmd::Update { name } => lifecycle::run_update(
            &marketplace_root,
            &name,
            humantime::format_rfc3339(std::time::SystemTime::now()).to_string(),
        )?,
        PluginCmd::Rollback { name } => lifecycle::run_rollback(&marketplace_root, &name)?,
        PluginCmd::Recover => recover::run(&marketplace_root)?,
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
