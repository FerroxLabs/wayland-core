//! CLI surface: `wayland-core migrate` — import an existing agent setup from
//! another tool into wayland-core's named profiles (issue #228).
//!
//! First slice is **Hermes-first** (see [`hermes`]): a Hermes profile
//! (`~/.hermes/profiles/<name>/`) maps ~1:1 onto a wayland-core named profile
//! (`[profiles.<name>]` in `config.toml`) — provider, model, base URL, the MCP
//! servers it references, and (opt-in) its provider API key.
//!
//! The importer follows the same discipline as the existing `legacy_import`
//! precedent in `wcore-memory`: it is **non-destructive** (never writes to the
//! source tree), **idempotent** (a profile whose name already exists is skipped
//! unless `--overwrite`), and reports exactly what it did. The flow is
//! detect → plan (preview) → confirm → apply, and `--dry-run` stops after the
//! preview.
//!
//! Skills, personas (`SOUL.md`), and long-term memory are detected and counted
//! in the preview but are NOT written in this slice — they are tracked for a
//! follow-up rather than silently dropped.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use wcore_config::config::{McpServerConfig, ProfileConfig, patch_global_config};

pub mod hermes;

/// `wayland-core migrate <source>` subcommands.
#[derive(Subcommand, Debug)]
pub enum MigrateCmd {
    /// Import Hermes profiles (`~/.hermes/profiles/*`) into wayland-core.
    Hermes(HermesArgs),
}

/// Options for `migrate hermes`.
#[derive(Args, Debug)]
pub struct HermesArgs {
    /// Hermes home to import from (default: `~/.hermes`).
    #[arg(long)]
    pub home: Option<PathBuf>,
    /// Show what would be imported and exit without writing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Apply without the interactive confirmation prompt.
    #[arg(long)]
    pub yes: bool,
    /// Also import provider API keys. Keys are written into `config.toml`
    /// (created `0600`). Off by default — secrets are never migrated silently.
    #[arg(long)]
    pub include_credentials: bool,
    /// Overwrite wayland-core profiles whose name already exists.
    #[arg(long)]
    pub overwrite: bool,
}

/// One wayland-core profile to be created from a source profile.
#[derive(Debug)]
pub struct ProfilePlan {
    /// Profile name (the `[profiles.<name>]` key).
    pub name: String,
    /// The mapped profile config. `api_key` is populated only when the caller
    /// asked to include credentials AND a provider key was found.
    pub config: ProfileConfig,
    /// A provider API key was found in the source `.env` (regardless of whether
    /// it will actually be written).
    pub has_credential: bool,
    /// The env var name the key came from — for the preview, never its value.
    pub credential_env_var: Option<String>,
    /// MCP server names this profile references.
    pub mcp_refs: Vec<String>,
    /// A wayland-core profile with this name already exists.
    pub conflict: bool,
}

/// Source artifacts detected but intentionally NOT imported in this slice.
#[derive(Debug, Default)]
pub struct Deferred {
    /// Skill directories found across the imported profiles.
    pub skills: usize,
    /// `SOUL.md` persona files found.
    pub personas: usize,
    /// Memory notes (`memories/*.md`, excluding the `MEMORY.md` entrypoint).
    pub memory_files: usize,
}

/// The full set of changes an import would make.
#[derive(Debug)]
pub struct MigrationPlan {
    /// Source tool name, e.g. `"hermes"`.
    pub source: &'static str,
    /// Resolved source home the plan was built from.
    pub source_home: PathBuf,
    /// Profiles to import (including ones flagged as conflicts).
    pub profiles: Vec<ProfilePlan>,
    /// New MCP server definitions to add, keyed by server name. Names already
    /// present in `config.toml` are excluded here and listed in
    /// [`MigrationPlan::mcp_conflicts`] instead.
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    /// MCP server names already present in `config.toml` (left untouched).
    pub mcp_conflicts: Vec<String>,
    /// Detected-but-deferred artifacts.
    pub deferred: Deferred,
    /// Non-fatal notes surfaced during planning.
    pub warnings: Vec<String>,
}

impl MigrationPlan {
    /// True when applying the plan would change nothing (every profile is a
    /// conflict that will be skipped, and there are no new MCP servers).
    fn is_empty(&self, overwrite: bool) -> bool {
        let no_profiles = self.profiles.iter().all(|p| p.conflict) && !overwrite;
        no_profiles && self.mcp_servers.is_empty()
    }
}

/// Summary of what an apply actually wrote.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub profiles_added: usize,
    pub profiles_skipped: usize,
    pub mcp_added: usize,
    pub credentials_written: usize,
}

/// Entry point for `wayland-core migrate`.
pub fn run(cmd: MigrateCmd) -> Result<()> {
    match cmd {
        MigrateCmd::Hermes(args) => run_hermes(args),
    }
}

fn run_hermes(args: HermesArgs) -> Result<()> {
    let home = hermes::detect_home(args.home.as_deref())?;
    let plan = hermes::build_plan(&home, args.include_credentials)?;

    render_plan(&plan, args.include_credentials, args.overwrite);

    if plan.is_empty(args.overwrite) {
        println!("\nNothing to import — every profile already exists.");
        return Ok(());
    }
    if args.dry_run {
        println!("\nDry run — no changes written.");
        return Ok(());
    }
    if !confirm(args.yes)? {
        println!("Aborted — no changes written.");
        return Ok(());
    }

    let report = apply_plan(&plan, args.include_credentials, args.overwrite)?;
    print_report(&report, &plan);
    Ok(())
}

/// Apply the plan to `config.toml` via the atomic partial writer.
///
/// A new profile is inserted whole. An EXISTING profile is only touched when
/// `overwrite` is set, and then it is **merged, not replaced**: the
/// importer-managed fields (provider, model, base URL, MCP refs) are refreshed
/// from the source, but a stored `api_key` is never wiped — it is replaced only
/// when this run both imports credentials AND actually found a new one — and
/// hand-added fields (`max_tokens`, `max_turns`, `extends`, `compat`) are left
/// intact. This keeps a previously-imported secret from silently vanishing on a
/// re-sync. MCP servers are always left untouched when the name collides.
fn apply_plan(
    plan: &MigrationPlan,
    include_credentials: bool,
    overwrite: bool,
) -> Result<MigrationReport> {
    let selected: Vec<&ProfilePlan> = plan
        .profiles
        .iter()
        .filter(|p| overwrite || !p.conflict)
        .collect();

    let credentials_written = if include_credentials {
        selected
            .iter()
            .filter(|p| p.config.api_key.is_some())
            .count()
    } else {
        0
    };
    let report = MigrationReport {
        profiles_added: selected.len(),
        profiles_skipped: plan.profiles.len() - selected.len(),
        mcp_added: plan.mcp_servers.len(),
        credentials_written,
    };

    patch_global_config(|f| {
        for pp in &selected {
            let incoming = &pp.config;
            match f.profiles.get_mut(&pp.name) {
                // Existing + `--overwrite`: merge, preserving secret & manual fields.
                Some(existing) if overwrite => {
                    existing.provider = incoming.provider.clone();
                    existing.model = incoming.model.clone();
                    existing.base_url = incoming.base_url.clone();
                    existing.mcp_servers = incoming.mcp_servers.clone();
                    if include_credentials && incoming.api_key.is_some() {
                        existing.api_key = incoming.api_key.clone();
                    }
                }
                // Existing without `--overwrite` (created between plan and
                // apply): leave it untouched — fail-safe skip.
                Some(_) => {}
                // Fresh profile.
                None => {
                    let mut cfg = incoming.clone();
                    if !include_credentials {
                        cfg.api_key = None;
                    }
                    f.profiles.insert(pp.name.clone(), cfg);
                }
            }
        }
        for (name, def) in &plan.mcp_servers {
            f.mcp
                .servers
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
    })?;

    Ok(report)
}

/// Prompt for confirmation. `--yes` skips it. A non-interactive stdin without
/// `--yes` is refused (fail-closed) rather than applied silently.
fn confirm(yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("refusing to apply without confirmation; re-run with --yes");
    }
    print!("Apply these changes? [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn render_plan(plan: &MigrationPlan, include_credentials: bool, overwrite: bool) {
    println!("Migration plan: {} → wayland-core", plan.source);
    println!("Source: {}", plan.source_home.display());
    println!("\nProfiles ({}):", plan.profiles.len());
    for p in &plan.profiles {
        let flag = match (p.conflict, overwrite) {
            (true, false) => "  [already exists — skipped unless --overwrite]",
            (true, true) => {
                "  [already exists — will be updated; existing credential & manual settings preserved]"
            }
            (false, _) => "",
        };
        println!("  • {}{}", p.name, flag);
        println!(
            "      provider={} model={}",
            p.config.provider.as_deref().unwrap_or("?"),
            p.config.model.as_deref().unwrap_or("?"),
        );
        if let Some(url) = &p.config.base_url {
            println!("      base_url={url}");
        }
        if !p.mcp_refs.is_empty() {
            println!("      mcp: {}", p.mcp_refs.join(", "));
        }
        if let Some(var) = &p.credential_env_var {
            if include_credentials {
                println!("      credential: {var} → config.toml (0600)");
            } else {
                println!(
                    "      credential: {var} found — NOT imported (pass --include-credentials)"
                );
            }
        }
    }

    if !plan.mcp_servers.is_empty() {
        let names: Vec<&str> = plan.mcp_servers.keys().map(String::as_str).collect();
        println!(
            "\nMCP servers to add ({}): {}",
            names.len(),
            names.join(", ")
        );
    }
    if !plan.mcp_conflicts.is_empty() {
        println!(
            "MCP servers already present (left untouched): {}",
            plan.mcp_conflicts.join(", ")
        );
    }

    let d = &plan.deferred;
    if d.skills + d.personas + d.memory_files > 0 {
        println!("\nDetected but NOT imported in this pass (tracked for a follow-up):");
        if d.skills > 0 {
            println!(
                "  • {} skill director{}",
                d.skills,
                plural(d.skills, "y", "ies")
            );
        }
        if d.personas > 0 {
            println!(
                "  • {} SOUL.md persona file{}",
                d.personas,
                plural(d.personas, "", "s")
            );
        }
        if d.memory_files > 0 {
            println!(
                "  • {} memory note{}",
                d.memory_files,
                plural(d.memory_files, "", "s")
            );
        }
    }
    for w in &plan.warnings {
        println!("  ! {w}");
    }
}

fn print_report(report: &MigrationReport, plan: &MigrationPlan) {
    println!(
        "\nImported {} profile{} ({} skipped), {} MCP server{}, {} credential{}.",
        report.profiles_added,
        plural(report.profiles_added, "", "s"),
        report.profiles_skipped,
        report.mcp_added,
        plural(report.mcp_added, "", "s"),
        report.credentials_written,
        plural(report.credentials_written, "", "s"),
    );
    let _ = plan;
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, conflict: bool) -> ProfilePlan {
        ProfilePlan {
            name: name.into(),
            config: ProfileConfig::default(),
            has_credential: false,
            credential_env_var: None,
            mcp_refs: Vec::new(),
            conflict,
        }
    }

    fn plan_with(profiles: Vec<ProfilePlan>) -> MigrationPlan {
        MigrationPlan {
            source: "hermes",
            source_home: PathBuf::from("/tmp/hermes"),
            profiles,
            mcp_servers: BTreeMap::new(),
            mcp_conflicts: Vec::new(),
            deferred: Deferred::default(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn is_empty_when_all_profiles_conflict_and_no_overwrite() {
        let plan = plan_with(vec![profile("a", true), profile("b", true)]);
        assert!(plan.is_empty(false));
        // With --overwrite the conflicting profiles are writable again.
        assert!(!plan.is_empty(true));
    }

    #[test]
    fn is_empty_false_when_a_fresh_profile_exists() {
        let plan = plan_with(vec![profile("a", true), profile("b", false)]);
        assert!(!plan.is_empty(false));
    }

    #[test]
    fn is_empty_false_when_only_new_mcp_servers() {
        let mut plan = plan_with(vec![profile("a", true)]);
        plan.mcp_servers.insert(
            "srv".into(),
            McpServerConfig {
                transport: wcore_config::config::TransportType::Stdio,
                command: Some("x".into()),
                args: None,
                env: None,
                url: None,
                headers: None,
                deferred: None,
                allow_local: false,
                only_for_assistant: None,
            },
        );
        assert!(!plan.is_empty(false));
    }
}
