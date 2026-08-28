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
//! Skills, personas (`SOUL.md`) and memory notes are **imported**, not merely
//! counted (see [`content`]). Data skills land live in the Wayland skills root;
//! personas and memory notes land staged, for reasons recorded on
//! [`content::ImportedContentStore::import_persona`] and
//! [`content::ImportedContentStore::import_memory_note`]. Executable content
//! goes to [`quarantine`] and stays inert until an operator promotes it.
//!
//! # Two content classes are deliberately NOT imported, and this is the reason
//!
//! Both are decisions, not omissions, and both are argued here so a reader does
//! not have to infer them from an absence:
//!
//! - **Peer settings.** The parts of a peer's configuration that have a Wayland
//!   equivalent — provider, model, base URL, MCP servers, credentials — are
//!   already imported, as profiles. What is left (OpenClaw's `flows/`,
//!   `tasks/`, `tui/`, `workspace/`, `identity/`, Hermes's non-`model:` keys)
//!   has **no Core semantics to map onto**, so importing it would mean guessing
//!   a mapping. Settings are the one content class where a wrong guess can
//!   *reduce* safety — approval mode, egress policy, sandbox posture and
//!   trust flags all live there — so an unmappable setting is reported in the
//!   deferred inventory and left on the peer's disk rather than approximated.
//! - **Assets.** Measured against the real peer home,
//!   `profiles/*/skins/**` holds **0 files**; no Core surface consumes a peer
//!   asset; and assets are the highest-byte, lowest-value class in a peer tree.
//!   A skill's OWN assets are imported, because they travel inside the skill
//!   directory and the skill is useless without them.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;
use wcore_config::config::{McpServerConfig, ProfileConfig, patch_global_config};
use wcore_config::portability::{
    CredentialRef, DiscoveredItem, ItemKind, PeerSource, PortabilityPlan, is_root_profile_id,
};

pub mod content;
pub mod gemini;
pub mod grok;
pub mod hermes;
pub mod openclaw;
pub mod provenance;
pub mod quarantine;
pub mod rollback;
pub mod select;

use content::{ContentRequest, ImportedContentStore};
use provenance::{PROVENANCE_FILE, Provenance, ProvenanceDocument};
use quarantine::{Classification, QuarantineRequest, QuarantineStore};
use select::{Accounting, Outcome, QuarantineReason, Selection};

/// `wayland-core migrate <source>` subcommands.
#[derive(Subcommand, Debug)]
pub enum MigrateCmd {
    /// Import Hermes profiles (`~/.hermes/profiles/*`) into wayland-core.
    Hermes(HermesArgs),
    /// Import an OpenClaw setup (`~/.openclaw`) into wayland-core.
    Openclaw(HermesArgs),
    /// Import a grok setup (`$GROK_HOME` or `~/.grok`) into wayland-core.
    Grok(HermesArgs),
    /// Import a gemini-cli setup (`~/.gemini`) into wayland-core.
    Gemini(HermesArgs),
    /// List imported content held in quarantine.
    Quarantined,
    /// Show the provenance of content this machine imported — what came from a
    /// peer, and where it landed.
    ///
    /// The complement of [`Quarantined`](Self::Quarantined): that answers
    /// "what was contained", this answers "where did the file in my skills
    /// directory come from". SC2 requires an import to PRESERVE provenance, and
    /// a record with no way to read it back preserves it only in the sense that
    /// a locked box preserves its contents.
    Imported(ImportedArgs),
    /// Promote quarantined content out of containment — the EXPLICIT OPERATOR
    /// ACTION, and the only thing that can.
    ///
    /// Nothing an imported artifact carries reaches this decision: the ids come
    /// from this command line and from nowhere else (see
    /// [`quarantine::QuarantineStore::promote`]). Promoting a whole set costs
    /// one invocation, so a realistic promotion does not cost one operator
    /// action per item.
    Promote(PromoteArgs),
}

/// Options for `migrate imported`.
#[derive(Args, Debug)]
pub struct ImportedArgs {
    /// Ask where ONE artifact came from, by its path.
    ///
    /// Accepts a path relative to the Wayland config dir or an absolute path
    /// under it, and a path INSIDE an imported directory
    /// (`skills/notes/SKILL.md`) as well as the directory itself — because the
    /// file is what an operator has in front of them when the question occurs
    /// to them.
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
    /// Emit the records as JSON instead of the prose listing.
    #[arg(long)]
    pub json: bool,
}

/// Options for `migrate promote`.
#[derive(Args, Debug)]
pub struct PromoteArgs {
    /// Identity of a quarantined item to promote. Repeatable; every id named
    /// is validated before anything moves, so a typo in a set cannot leave a
    /// half-applied promotion.
    #[arg(long = "id", value_name = "IDENTITY", required_unless_present = "all")]
    pub ids: Vec<String>,
    /// Promote everything currently in quarantine.
    #[arg(long)]
    pub all: bool,
}

/// Options for `migrate hermes` and `migrate openclaw`.
///
/// Shared deliberately: the two sources differ in what they READ, not in how a
/// user drives them, and a second identical arg struct would drift.
#[derive(Args, Debug)]
pub struct HermesArgs {
    /// Source home to import from (default: `~/.hermes` or `~/.openclaw`).
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
    /// Emit the plan as machine-readable JSON instead of the prose preview.
    ///
    /// The emitted document is the typed plan, in which a credential value is
    /// unrepresentable — only its name and source file appear. Implies a
    /// preview: with `--json` nothing is ever written.
    #[arg(long)]
    pub json: bool,
    /// Import ONLY these item identities (repeatable). The identities are the
    /// ones the dry-run plan published, so a user selects from what they
    /// previewed. An identity the plan did not publish is REFUSED, not ignored.
    #[arg(long = "select", value_name = "IDENTITY")]
    pub select: Vec<String>,
    /// Import everything EXCEPT these item identities (repeatable). Same
    /// identity vocabulary, same refusal on an unpublished id.
    #[arg(long = "exclude", value_name = "IDENTITY")]
    pub exclude: Vec<String>,
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
    /// The file the credential was found in, relative to the source home.
    pub credential_file: Option<String>,
    /// MCP server names this profile references.
    pub mcp_refs: Vec<String>,
    /// A wayland-core profile with this name already exists.
    pub conflict: bool,
    /// Where this setup came from, relative to the source home.
    pub source_path: String,
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
    /// Source-specific detected-but-not-imported counts, keyed by kind. The
    /// OpenClaw tree carries several categories Hermes has no equivalent for
    /// (per-agent state, flows, tasks, identity, …); they are COUNTED here
    /// rather than dropped, because a discovered item that is neither imported
    /// nor named has been silently lost.
    pub deferred_other: BTreeMap<String, usize>,
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

    /// Project onto the typed, structurally-redacted plan that `--json` emits.
    ///
    /// **This conversion is the redaction boundary.** `MigrationPlan` can hold a
    /// real `api_key` — it has to, because `--include-credentials` writes one —
    /// and [`PortabilityPlan`] cannot. The value is dropped HERE and there is no
    /// inverse conversion, so a consumer handed the emitted plan cannot render a
    /// secret through `serde`, `Debug`, `Display` or an error formatter even
    /// deliberately. Nothing below reads `config.api_key`.
    pub fn to_portability(&self) -> PortabilityPlan {
        let source = match self.source {
            "openclaw" => PeerSource::OpenClaw,
            "grok" => PeerSource::Grok,
            "gemini" => PeerSource::Gemini,
            _ => PeerSource::Hermes,
        };
        let mut out = PortabilityPlan::new(source, self.source_home.display().to_string());

        for p in &self.profiles {
            let kind = if is_root_profile_id(&p.name) {
                ItemKind::RootProfile
            } else {
                ItemKind::Profile
            };
            let mut item = DiscoveredItem::new(
                kind,
                p.name.clone(),
                p.source_path.clone(),
                format!("profiles.{}", p.name),
            );
            item.conflict = p.conflict;
            // Reference only — by TYPE there is nowhere for a value to go.
            item.credential = p.credential_env_var.as_ref().map(|name| {
                CredentialRef::new(name.clone(), p.credential_file.clone().unwrap_or_default())
            });
            if let Some(v) = &p.config.provider {
                item.insert_detail("provider", v);
            }
            if let Some(v) = &p.config.model {
                item.insert_detail("model", v);
            }
            if let Some(v) = &p.config.base_url {
                item.insert_detail("base_url", v);
            }
            if !p.mcp_refs.is_empty() {
                item.insert_detail("mcp_refs", &p.mcp_refs.join(","));
            }
            out.items.push(item);
        }

        for (name, srv) in &self.mcp_servers {
            let mut item = DiscoveredItem::new(
                ItemKind::McpServer,
                name.clone(),
                String::new(),
                format!("mcp.servers.{name}"),
            );
            item.insert_detail("transport", &format!("{:?}", srv.transport));
            if let Some(c) = &srv.command {
                item.insert_detail("command", c);
            }
            if let Some(u) = &srv.url {
                item.insert_detail("url", u);
            }
            out.items.push(item);
        }

        // F26-GRADE-H1: skills, personas and memory notes are no longer
        // reported as deferred, because they are no longer deferred — each is
        // discovered by `ImportSurface`, published with a selectable identity,
        // and written. Emitting them here as well would restate the original
        // defect in the machine-readable document: one run, two incompatible
        // claims about the same items. `MigrationPlan::deferred` is retained as
        // the SOURCE-side inventory the mappers build (and the hermes mapper's
        // tests assert), not as a claim about what this import skipped.
        for (k, v) in &self.deferred_other {
            if *v > 0 {
                out.deferred.insert(k.clone(), *v);
            }
        }

        out.warnings = self.warnings.clone();
        for name in &self.mcp_conflicts {
            out.warnings.push(format!(
                "mcp server {name:?} already exists — left untouched"
            ));
        }
        out.finalize();
        out
    }
}

/// Summary of what an apply actually wrote.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub profiles_added: usize,
    pub profiles_skipped: usize,
    pub mcp_added: usize,
    pub credentials_written: usize,
    /// How many discovered items were contained rather than imported. Reported
    /// rather than silent: a containment the user never learns about is
    /// discovered later as missing functionality and worked around.
    pub quarantined: usize,
    /// Each contained item as `identity — reason`.
    pub quarantine_notices: Vec<String>,
    /// The four counts the conservation invariant is asserted over:
    /// `imported + quarantined + excluded == discovered`.
    pub discovered: usize,
    pub imported: usize,
    pub excluded: usize,
    /// **Files this run actually wrote into the Wayland home**, taken from the
    /// writer's own counter rather than from the item count.
    ///
    /// Added for F26-GRADE-H1. A per-item count can be inflated by an outcome
    /// recorded beside a write that never happened — which is exactly the
    /// defect this field exists to make impossible to hide: if the categories
    /// below are non-zero and this is zero, the report contradicts itself
    /// visibly instead of agreeing with itself falsely.
    pub files_written: usize,
    /// Data skills written into the live skills root.
    pub skills_imported: usize,
    /// Persona bodies written into the staging root.
    pub personas_imported: usize,
    /// Memory notes written into the staging root.
    pub memory_imported: usize,
    /// Skills whose content was byte-identical to one already written this run,
    /// so they were recorded rather than duplicated on disk. Counted in
    /// `skills_imported`; reported separately so the file count and the item
    /// count can be reconciled by a reader.
    pub skills_deduplicated: usize,
    /// Imported files whose SOURCE carried a POSIX execute bit, which this
    /// import removed.
    ///
    /// Measured against the real peer trees: **68 of 349 peer skills carry a
    /// `.sh`/`.py`/`.js` helper or an execute-bit file**, and
    /// `classify_skill_body` reads only the SKILL.md prose, so all of them
    /// classify `Data` and land live. Wayland does not auto-run them — its one
    /// auto-execution surface is the `` ```! `` directive, which IS classified
    /// and contained — but a peer script arriving at `0755` is one `./script`
    /// away from running with no containment decision ever having been made.
    /// So the bit is removed and the count is surfaced.
    pub exec_bits_stripped: usize,
}

/// Running totals of what the content writer actually put on disk.
#[derive(Debug, Default)]
struct ContentTally {
    skills: usize,
    personas: usize,
    memory: usize,
    deduplicated: usize,
}

impl MigrationReport {
    /// The conservation invariant as a predicate over the REPORTED numbers, so
    /// a caller (or a test, or the scale measurement) checks the same
    /// arithmetic the user was shown rather than an internal one.
    pub fn balances(&self) -> bool {
        self.imported + self.quarantined + self.excluded == self.discovered
    }
}

/// One selectable, published item identity and how it was classified.
///
/// Identity is `kind:id`, qualified so a profile named `srv` and an MCP server
/// named `srv` are two identities rather than one. The same string is published
/// in the dry-run document, addressed by `--select` / `--exclude`, keyed in the
/// quarantine index, and keyed in the provenance record — ONE scheme, so a
/// later selective rollback addresses exactly what an import addressed.
#[derive(Debug, Clone, Serialize)]
pub struct PublishedItem {
    pub identity: String,
    pub source_path: String,
    /// `data` or `executable`.
    pub class: &'static str,
    /// Present only for executable items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_reason: Option<String>,
}

/// The document `migrate --json` emits.
///
/// 26-01's [`PortabilityPlan`] is flattened in UNCHANGED — this plan consumes
/// that vocabulary rather than reshaping it — and two additive keys publish
/// what this plan adds: the selectable identity of every discovered item, and
/// what would be quarantined.
#[derive(Debug, Serialize)]
struct ImportPlanDocument {
    #[serde(flatten)]
    plan: PortabilityPlan,
    /// Every discovered item, by identity. `--select` / `--exclude` address
    /// exactly these strings, so a user selects from what they previewed.
    published: Vec<PublishedItem>,
    /// Identities that would land in quarantine, with why.
    would_quarantine: Vec<PublishedItem>,
}

/// Everything this plan discovers beyond 26-01's profile/MCP vocabulary.
///
/// Before F26-GRADE-H1 this held exactly one field, which is why persona,
/// memory, settings and asset content all measured **0 files written** against
/// a real 13-profile peer home. Skills, personas and memory notes are now each
/// discovered, published, selectable, and — the part that was missing —
/// actually written (see [`content`]).
struct ImportSurface {
    /// Peer skill directories, with the classification the EXISTING detector
    /// gave each one.
    skills: Vec<(quarantine::ScannedExecutable, Classification)>,
    /// Peer persona bodies (`SOUL.md`). Data by construction — a persona is
    /// prose — but staged rather than activated; see
    /// [`ImportedContentStore::import_persona`].
    personas: Vec<quarantine::ScannedData>,
    /// Peer memory notes. Data, staged for the structural reason recorded on
    /// [`ImportedContentStore::import_memory_note`].
    memory: Vec<quarantine::ScannedData>,
}

impl ImportSurface {
    fn scan(home: &std::path::Path) -> Self {
        Self {
            skills: quarantine::scan_peer_skills(home),
            personas: quarantine::scan_peer_personas(home),
            memory: quarantine::scan_peer_memory(home),
        }
    }
}

/// The qualified identity of a discovered profile / root profile / MCP server.
fn plan_identity(kind: ItemKind, id: &str) -> String {
    let k = match kind {
        ItemKind::RootProfile => "root_profile",
        ItemKind::Profile => "profile",
        ItemKind::McpServer => "mcp_server",
    };
    format!("{k}:{id}")
}

/// Every identity the plan publishes, in a stable order.
fn published_items(plan: &MigrationPlan, surface: &ImportSurface) -> Vec<PublishedItem> {
    let mut out = Vec::new();
    for p in &plan.profiles {
        let kind = if is_root_profile_id(&p.name) {
            ItemKind::RootProfile
        } else {
            ItemKind::Profile
        };
        out.push(PublishedItem {
            identity: plan_identity(kind, &p.name),
            source_path: p.source_path.clone(),
            class: "data",
            executable_reason: None,
        });
    }
    for (name, srv) in &plan.mcp_servers {
        let class = quarantine::classify_mcp_server(srv);
        out.push(PublishedItem {
            identity: plan_identity(ItemKind::McpServer, name),
            source_path: String::new(),
            class: if class.is_executable() {
                "executable"
            } else {
                "data"
            },
            executable_reason: class.reason().map(|r| r.to_string()),
        });
    }
    for (found, class) in &surface.skills {
        out.push(PublishedItem {
            identity: found.id.clone(),
            source_path: found.relative.clone(),
            class: if class.is_executable() {
                "executable"
            } else {
                "data"
            },
            executable_reason: class.reason().map(|r| r.to_string()),
        });
    }
    // Personas and memory notes are prose: data by construction, and published
    // so they are selectable by the SAME identity vocabulary everything else
    // uses rather than importing invisibly.
    for d in surface.personas.iter().chain(surface.memory.iter()) {
        out.push(PublishedItem {
            identity: d.id.clone(),
            source_path: d.relative.clone(),
            class: "data",
            executable_reason: None,
        });
    }
    out.sort_by(|a, b| a.identity.cmp(&b.identity));
    out
}

/// Run a full import and RETURN the report.
///
/// This is the same path the CLI takes — [`run_source`] calls it and then
/// prints the result — exposed so a caller can assert the arithmetic the user
/// was actually shown rather than recomputing a parallel one that could
/// disagree with it. No prompting and no rendering happen here.
pub fn run_import(source: PeerSource, args: &HermesArgs) -> Result<MigrationReport> {
    let (home, plan) = detect_and_plan(source, args)?;
    let surface = ImportSurface::scan(&home);
    let published = published_items(&plan, &surface);
    let selection = Selection::from_flags(&args.select, &args.exclude);
    let ids: Vec<String> = published.iter().map(|p| p.identity.clone()).collect();
    selection.resolve(&ids)?;
    apply_plan(
        &plan,
        &surface,
        &published,
        &selection,
        source,
        args.include_credentials,
        args.overwrite,
    )
}

/// The published identities for a source, without applying anything — what a
/// user would select from after a dry run.
pub fn published_for(source: PeerSource, args: &HermesArgs) -> Result<Vec<PublishedItem>> {
    let (home, plan) = detect_and_plan(source, args)?;
    let surface = ImportSurface::scan(&home);
    Ok(published_items(&plan, &surface))
}

fn detect_and_plan(source: PeerSource, args: &HermesArgs) -> Result<(PathBuf, MigrationPlan)> {
    match source {
        PeerSource::Hermes => {
            let home = hermes::detect_home(args.home.as_deref())?;
            let plan = hermes::build_plan(&home, args.include_credentials)?;
            Ok((home, plan))
        }
        PeerSource::OpenClaw => {
            let home = openclaw::detect_home(args.home.as_deref())?;
            let plan = openclaw::build_plan(&home, args.include_credentials)?;
            Ok((home, plan))
        }
        PeerSource::Grok => {
            let home = grok::detect_home(args.home.as_deref())?;
            let plan = grok::build_plan(&home, args.include_credentials)?;
            Ok((home, plan))
        }
        PeerSource::Gemini => {
            let home = gemini::detect_home(args.home.as_deref())?;
            let plan = gemini::build_plan(&home, args.include_credentials)?;
            Ok((home, plan))
        }
    }
}

/// Entry point for `wayland-core migrate`.
pub fn run(cmd: MigrateCmd) -> Result<()> {
    match cmd {
        MigrateCmd::Hermes(args) => run_source(PeerSource::Hermes, args),
        MigrateCmd::Openclaw(args) => run_source(PeerSource::OpenClaw, args),
        MigrateCmd::Grok(args) => run_source(PeerSource::Grok, args),
        MigrateCmd::Gemini(args) => run_source(PeerSource::Gemini, args),
        MigrateCmd::Quarantined => run_quarantined(),
        MigrateCmd::Imported(args) => run_imported(args),
        MigrateCmd::Promote(args) => run_promote(args),
    }
}

/// `migrate quarantined` — what is contained, and why.
fn run_quarantined() -> Result<()> {
    let store = QuarantineStore::for_current_home();
    let entries = store.entries()?;
    if entries.is_empty() {
        println!("Nothing is quarantined.");
        return Ok(());
    }
    println!("Quarantined imports ({}):", entries.len());
    for e in &entries {
        println!("  • {}", e.id);
        println!("      reason: {}", e.reason);
        println!(
            "      from:   {} ({})",
            e.provenance.source_path, e.provenance.source_tool
        );
        println!("      digest: {}", e.provenance.digest);
    }
    println!("\nPromote with: wayland-core migrate promote --id <identity> [--id …]");
    Ok(())
}

/// Load the union of every provenance record this home holds.
///
/// Two stores, one vocabulary. The imported document
/// (`migrate-imported/PROVENANCE.json`) and the quarantine index both key by
/// the SAME published identity and both now carry a home-relative
/// `written_path`, so a single lookup answers for live, staged and contained
/// content alike. Reading only one of them would answer "where did this come
/// from?" with a confident "nowhere" for half the artifacts on disk — the
/// known-negative failure this program keeps re-finding.
pub fn imported_provenance(home: &std::path::Path) -> Result<ProvenanceDocument> {
    let mut doc = ProvenanceDocument::new();
    let staged = home.join(content::IMPORT_STAGE_DIR).join(PROVENANCE_FILE);
    if staged.is_file() {
        let text = std::fs::read_to_string(&staged)
            .with_context(|| format!("reading {}", staged.display()))?;
        let loaded = ProvenanceDocument::from_json(&text)
            .with_context(|| format!("parsing {}", staged.display()))?;
        for (id, p) in loaded.entries {
            doc.insert(id, p);
        }
    }
    let store = QuarantineStore::new(home.join(quarantine::QUARANTINE_DIR));
    for e in store.entries().unwrap_or_default() {
        doc.insert(e.id.clone(), e.provenance.clone());
    }
    Ok(doc)
}

/// `migrate imported` — where did the content on this machine come from?
fn run_imported(args: ImportedArgs) -> Result<()> {
    let home = wcore_config::config::wayland_config_dir();
    let doc = imported_provenance(&home)?;

    if let Some(path) = &args.path {
        // Accept an absolute path under the home as readily as a relative one:
        // the operator has a path from `ls`, not a path in our coordinates.
        let rel = path
            .strip_prefix(&home)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .to_string();
        let hits = doc.resolve_path(&rel);
        if args.json {
            let out: Vec<_> = hits
                .iter()
                .map(|(id, p)| serde_json::json!({ "identity": id, "provenance": p }))
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }
        if hits.is_empty() {
            // An honest "no record", NOT a guess. This is the negative answer
            // the command must be able to give: a path Wayland's own user
            // authored has no provenance, and saying so is the whole value of
            // being able to ask.
            println!("No import record covers {}", path.display());
            println!("Nothing this machine imported from a peer landed there.");
            return Ok(());
        }
        println!("{} came from:", path.display());
        for (id, p) in &hits {
            print_provenance(id, p);
        }
        return Ok(());
    }

    if args.json {
        println!("{}", doc.to_json()?);
        return Ok(());
    }
    if doc.is_empty() {
        println!("No peer content has been imported into this home.");
        return Ok(());
    }
    println!(
        "Imported content ({} item{}):",
        doc.len(),
        plural(doc.len(), "", "s")
    );
    for (id, p) in &doc.entries {
        print_provenance(id, p);
    }
    // A record that names no destination is reported rather than rendered as
    // though it were complete — the F26-GRADE-H1 shape one level up.
    let orphans = doc.without_destination();
    if !orphans.is_empty() {
        println!(
            "\n{} record{} name no destination and cannot be located on disk: {}",
            orphans.len(),
            plural(orphans.len(), "", "s"),
            orphans.join(", ")
        );
    }
    Ok(())
}

fn print_provenance(id: &str, p: &Provenance) {
    println!("  • {id}");
    println!("      from:  {} ({})", p.source_path, p.source_tool);
    match &p.written_path {
        Some(w) => println!("      at:    {w}"),
        None => println!("      at:    (no destination recorded)"),
    }
    if let Some(first) = &p.deduplicated_with {
        println!("      note:  identical content, written by {first}");
    }
    println!("      digest: {}", p.digest);
}

/// `migrate promote` — THE explicit operator action.
///
/// The identities come from this command line. Nothing is read out of the
/// quarantined payload to reach this decision; see
/// [`quarantine::QuarantineStore::promote`].
fn run_promote(args: PromoteArgs) -> Result<()> {
    let store = QuarantineStore::for_current_home();
    let ids: Vec<String> = if args.all {
        store.entries()?.into_iter().map(|e| e.id).collect()
    } else {
        args.ids.clone()
    };
    if ids.is_empty() {
        println!("Nothing to promote.");
        return Ok(());
    }
    let dest = wcore_config::config::wayland_config_dir().join("skills");
    let promoted = store.promote(&ids, &dest)?;
    println!(
        "Promoted {} item{} out of quarantine into {} (1 operator invocation).",
        promoted.len(),
        plural(promoted.len(), "", "s"),
        dest.display()
    );
    let renamed = promoted.iter().filter(|p| p.renamed).count();
    for p in &promoted {
        if p.renamed {
            println!("  • {} → {} (name already taken)", p.id, p.promoted_as);
        } else {
            println!("  • {}", p.id);
        }
    }
    if renamed > 0 {
        println!(
            "\n{renamed} item{} landed under a disambiguated name because a peer install \
             reuses one skill name across profiles. Nothing was overwritten.",
            plural(renamed, "", "s")
        );
    }
    Ok(())
}

fn run_source(source: PeerSource, args: HermesArgs) -> Result<()> {
    let (home, plan) = detect_and_plan(source, &args)?;

    let surface = ImportSurface::scan(&home);
    let published = published_items(&plan, &surface);
    let selection = Selection::from_flags(&args.select, &args.exclude);
    // Refuse an identity the plan never published, BEFORE anything is written.
    // A typo that quietly imports nothing is a user telling the tool to do
    // something and being told it succeeded.
    let ids: Vec<String> = published.iter().map(|p| p.identity.clone()).collect();
    selection.resolve(&ids)?;

    // `--json` is a PREVIEW surface: it emits the typed plan and never writes,
    // so an unconfirmed apply cannot mutate anything through it.
    if args.json {
        let doc = ImportPlanDocument {
            plan: plan.to_portability(),
            would_quarantine: published
                .iter()
                .filter(|p| p.class == "executable" && selection.wants(&p.identity))
                .cloned()
                .collect(),
            published,
        };
        println!("{}", serde_json::to_string_pretty(&doc)?);
        return Ok(());
    }

    render_plan(&plan, args.include_credentials, args.overwrite);
    render_containment(&published, &selection);

    if plan.is_empty(args.overwrite) && !published.iter().any(|p| p.class == "executable") {
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

    // SC3 / G3. Everything from here to `commit()` is undoable. The journal is
    // opened AFTER the confirmation and the dry-run exits, because those paths
    // write nothing and a journal over a no-op would only add bookkeeping to a
    // home that was never touched.
    let home_dir = wcore_config::config::wayland_config_dir();
    let guard = rollback::ApplyGuard::open(&home_dir)?;
    if guard.recovered_before_start() > 0 {
        println!(
            "Rolled back {} interrupted import{} before starting; your home is back \
             to the state it was in before that run.",
            guard.recovered_before_start(),
            plural(guard.recovered_before_start(), "", "s")
        );
    }

    let report = match apply_plan(
        &plan,
        &surface,
        &published,
        &selection,
        source,
        args.include_credentials,
        args.overwrite,
    ) {
        Ok(report) => report,
        Err(e) => {
            // A failed apply is an interruption with a return value. Rolling
            // back here is what stops a half-written import from becoming the
            // home's new state; the rollback error, if any, is chained so a
            // failure to undo can never be mistaken for a clean abort.
            guard.rollback()?;
            return Err(e);
        }
    };
    guard.commit()?;
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
#[allow(clippy::too_many_arguments)]
fn apply_plan(
    plan: &MigrationPlan,
    surface: &ImportSurface,
    published: &[PublishedItem],
    selection: &Selection,
    source: PeerSource,
    include_credentials: bool,
    overwrite: bool,
) -> Result<MigrationReport> {
    // Every published identity gets EXACTLY ONE outcome. `Accounting` is keyed
    // by identity, so an item cannot hold two outcomes and cannot be counted
    // twice; anything that never gets one is named by `unaccounted()`.
    let mut acct = Accounting::over(published.iter().map(|p| p.identity.clone()));
    let store = QuarantineStore::for_current_home();
    let mut content = ImportedContentStore::for_current_home();
    let mut written = ContentTally::default();

    // --- containment first, so nothing executable can be written live -------
    let mut contained: Vec<String> = Vec::new();
    for (found, class) in &surface.skills {
        if !selection.wants(&found.id) {
            acct.record(&found.id, Outcome::Excluded);
            continue;
        }
        match class {
            Classification::Data => {
                // A skill body with no directive is not a shell surface, so it
                // imports without ceremony — treating everything as dangerous
                // trains an operator to promote without reading.
                //
                // F26-GRADE-H1: the outcome is recorded FROM THE WRITE, not
                // beside it. Before this, `Outcome::Imported` was recorded here
                // with no write of any kind, so `imported=` counted content the
                // filesystem did not hold and the same run also printed those
                // items under "Detected but NOT imported". A failed write is now
                // a NAMED failure that still balances — never a silent success.
                let req = ContentRequest {
                    id: found.id.clone(),
                    source_dir: Some(found.dir.clone()),
                    inline: None,
                    source_tool: source.as_str().to_string(),
                    source_version: peer_version(&plan.source_home, source),
                    source_path: found.relative.clone(),
                    name: found.name.clone(),
                };
                match content.import_skill(&req) {
                    Ok(item) => {
                        written.skills += 1;
                        if item.deduplicated_with.is_some() {
                            written.deduplicated += 1;
                        }
                        acct.record(&found.id, Outcome::Imported);
                    }
                    Err(e) => {
                        acct.record(
                            &found.id,
                            Outcome::Quarantined(QuarantineReason::ImportFailed(e.to_string())),
                        );
                        contained.push(format!("{} — refused: {e}", found.id));
                    }
                }
            }
            Classification::Executable(reason) => {
                let req = QuarantineRequest {
                    id: found.id.clone(),
                    reason: *reason,
                    source_dir: Some(found.dir.clone()),
                    inline: None,
                    source_tool: source.as_str().to_string(),
                    source_version: peer_version(&plan.source_home, source),
                    source_path: found.relative.clone(),
                    promote_as: found.name.clone(),
                };
                match store.admit(&req) {
                    Ok(_) => {
                        acct.record(
                            &found.id,
                            Outcome::Quarantined(QuarantineReason::Executable(reason.to_string())),
                        );
                        contained.push(format!("{} — {reason}", found.id));
                    }
                    Err(e) => {
                        // A refusal (symlink, oversized file, oversized
                        // surface) is a NAMED failure that still balances —
                        // never a silent drop, and never a live import.
                        acct.record(
                            &found.id,
                            Outcome::Quarantined(QuarantineReason::ImportFailed(e.to_string())),
                        );
                        contained.push(format!("{} — refused: {e}", found.id));
                    }
                }
            }
        }
    }

    // --- personas and memory notes: prose, imported STAGED ------------------
    // Both are data, and both are written rather than counted — the whole point
    // of F26-GRADE-H1. They land staged rather than live because Core has no
    // destination for either without a decision only the operator holds; the
    // full argument is on `content::ImportedContentStore::import_persona` and
    // `::import_memory_note`. As with skills, the outcome comes from the write.
    for (items, is_persona) in [(&surface.personas, true), (&surface.memory, false)] {
        for d in items {
            if !selection.wants(&d.id) {
                acct.record(&d.id, Outcome::Excluded);
                continue;
            }
            let body = match std::fs::read(&d.path) {
                Ok(b) => b,
                Err(e) => {
                    acct.record(
                        &d.id,
                        Outcome::Quarantined(QuarantineReason::ImportFailed(e.to_string())),
                    );
                    contained.push(format!("{} — refused: {e}", d.id));
                    continue;
                }
            };
            let req = ContentRequest {
                id: d.id.clone(),
                source_dir: None,
                inline: Some((d.name.clone(), body)),
                source_tool: source.as_str().to_string(),
                source_version: peer_version(&plan.source_home, source),
                source_path: d.relative.clone(),
                name: d.name.clone(),
            };
            let result = if is_persona {
                content.import_persona(&req)
            } else {
                content.import_memory_note(&req)
            };
            match result {
                // The count is incremented only on a successful write, and the
                // authoritative file total still comes from the store's own
                // counter (`content.files_written()`) rather than from here —
                // two independent numbers that a reader can cross-check, which
                // is the property F26-GRADE-H1 was missing.
                Ok(_) => {
                    if is_persona {
                        written.personas += 1;
                    } else {
                        written.memory += 1;
                    }
                    acct.record(&d.id, Outcome::Imported);
                }
                Err(e) => {
                    acct.record(
                        &d.id,
                        Outcome::Quarantined(QuarantineReason::ImportFailed(e.to_string())),
                    );
                    contained.push(format!("{} — refused: {e}", d.id));
                }
            }
        }
    }

    // --- MCP: a launch command is a child process, not a setting ------------
    let mut mcp_live: BTreeMap<String, McpServerConfig> = BTreeMap::new();
    // Names that will NOT exist in `[mcp.servers]` after this apply, so an
    // imported profile must not keep referencing them. A dangling reference is
    // not merely untidy: if a server of that name is defined later, the profile
    // silently picks it up — the containment decision made here would then be
    // quietly undone by an unrelated future edit.
    let mut mcp_withheld: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (name, srv) in &plan.mcp_servers {
        let identity = plan_identity(ItemKind::McpServer, name);
        if !selection.wants(&identity) {
            acct.record(&identity, Outcome::Excluded);
            mcp_withheld.insert(name.clone());
            continue;
        }
        match quarantine::classify_mcp_server(srv) {
            Classification::Executable(reason) => {
                let body = serde_json::to_vec_pretty(srv).unwrap_or_default();
                let req = QuarantineRequest {
                    id: identity.clone(),
                    reason,
                    source_dir: None,
                    inline: Some(("mcp-server.json".to_string(), body)),
                    source_tool: source.as_str().to_string(),
                    source_version: peer_version(&plan.source_home, source),
                    source_path: format!("mcp_servers/{name}"),
                    promote_as: name.clone(),
                };
                mcp_withheld.insert(name.clone());
                match store.admit(&req) {
                    Ok(_) => {
                        acct.record(
                            &identity,
                            Outcome::Quarantined(QuarantineReason::Executable(reason.to_string())),
                        );
                        contained.push(format!("{identity} — {reason}"));
                    }
                    Err(e) => {
                        acct.record(
                            &identity,
                            Outcome::Quarantined(QuarantineReason::ImportFailed(e.to_string())),
                        );
                        contained.push(format!("{identity} — refused: {e}"));
                    }
                }
            }
            Classification::Data => {
                mcp_live.insert(name.clone(), srv.clone());
                acct.record(&identity, Outcome::Imported);
            }
        }
    }

    // --- profiles: data, imported without ceremony --------------------------
    let selected: Vec<&ProfilePlan> = plan
        .profiles
        .iter()
        .filter(|p| {
            let kind = if is_root_profile_id(&p.name) {
                ItemKind::RootProfile
            } else {
                ItemKind::Profile
            };
            selection.wants(&plan_identity(kind, &p.name))
        })
        .filter(|p| overwrite || !p.conflict)
        .collect();
    for p in &plan.profiles {
        let kind = if is_root_profile_id(&p.name) {
            ItemKind::RootProfile
        } else {
            ItemKind::Profile
        };
        let identity = plan_identity(kind, &p.name);
        if !selection.wants(&identity) {
            acct.record(&identity, Outcome::Excluded);
        } else if selected.iter().any(|s| s.name == p.name) {
            acct.record(&identity, Outcome::Imported);
        } else {
            // A conflict skipped without `--overwrite` is a named failure, not
            // a vanished item: it still appears in the arithmetic.
            acct.record(
                &identity,
                Outcome::Quarantined(QuarantineReason::ImportFailed(
                    "a wayland-core profile of this name already exists; re-run with --overwrite"
                        .into(),
                )),
            );
            contained.push(format!("{identity} — already exists, left untouched"));
        }
    }

    let credentials_written = if include_credentials {
        selected
            .iter()
            .filter(|p| p.config.api_key.is_some())
            .count()
    } else {
        0
    };
    let (discovered, imported, quarantined, excluded) = acct.counts();
    // The conservation invariant, as arithmetic, at the moment of the apply.
    // An unbalanced accounting means an item was lost between discovery and
    // outcome, which is the data loss this exists to catch — so it is a hard
    // error rather than a warning nobody reads.
    if !acct.balances() {
        bail!(
            "internal accounting did not balance: discovered={discovered} \
             imported={imported} quarantined={quarantined} excluded={excluded}; \
             unaccounted={:?} undiscovered={:?}",
            acct.unaccounted(),
            acct.undiscovered()
        );
    }
    // Persist the run's provenance for everything that landed. Done BEFORE the
    // config patch so an interruption cannot leave written content with no
    // record of where it came from — an imported item with no provenance cannot
    // be selectively rolled back or judged when its source is later found
    // malicious, which is the property `provenance.rs` exists to hold.
    content
        .flush()
        .map_err(|e| anyhow::anyhow!("recording import provenance failed: {e}"))?;

    let report = MigrationReport {
        profiles_added: selected.len(),
        profiles_skipped: plan.profiles.len() - selected.len(),
        mcp_added: mcp_live.len(),
        credentials_written,
        quarantined,
        quarantine_notices: contained,
        discovered,
        imported,
        excluded,
        files_written: content.files_written(),
        skills_imported: written.skills,
        personas_imported: written.personas,
        memory_imported: written.memory,
        skills_deduplicated: written.deduplicated,
        exec_bits_stripped: content.exec_bits_stripped(),
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
                    existing.mcp_servers = strip_withheld(&incoming.mcp_servers, &mcp_withheld);
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
                    cfg.mcp_servers = strip_withheld(&incoming.mcp_servers, &mcp_withheld);
                    if !include_credentials {
                        cfg.api_key = None;
                    }
                    f.profiles.insert(pp.name.clone(), cfg);
                }
            }
        }
        // ONLY the non-executable MCP definitions. A peer entry carrying a
        // launch command never reaches `config.toml`, because writing it there
        // makes it launchable — the child-process surface T-26-02-03 names.
        for (name, def) in &mcp_live {
            f.mcp
                .servers
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
    })?;

    Ok(report)
}

/// Drop references to MCP servers this apply withheld, so an imported profile
/// never names a server that is not defined.
fn strip_withheld(
    refs: &Option<Vec<String>>,
    withheld: &std::collections::BTreeSet<String>,
) -> Option<Vec<String>> {
    refs.as_ref().map(|list| {
        list.iter()
            .filter(|n| !withheld.contains(n.as_str()))
            .cloned()
            .collect()
    })
}

/// The version the SOURCE declares, if it declares one.
///
/// Read from the peer's own state — never invented. `None` is an honest
/// absence: a fabricated version in a provenance record is worse than no
/// version, because it reads as a fact.
fn peer_version(home: &std::path::Path, source: PeerSource) -> Option<String> {
    let candidates: &[&str] = match source {
        PeerSource::Hermes => &["VERSION", "version"],
        PeerSource::OpenClaw => &["VERSION", "version"],
        // grok records the installed channel/version beside its binary rather
        // than in a root VERSION file; the plain names are still probed because
        // a packaged install may drop one, and a miss is an honest `None`.
        PeerSource::Grok => &["VERSION", "version"],
        PeerSource::Gemini => &["VERSION", "version"],
    };
    for name in candidates {
        if let Ok(s) = std::fs::read_to_string(home.join(name)) {
            let t = s.trim();
            if !t.is_empty() && t.len() <= 64 {
                return Some(t.to_string());
            }
        }
    }
    // grok records its installed version in `version.json`, NOT in a plain
    // `VERSION` file. Found by driving the real `~/.grok` on this machine —
    // the probe list above returned `None` against a home that plainly declares
    // `"version": "0.2.103"`, which is the honest-absence rule turning into a
    // silently missing fact.
    if matches!(source, PeerSource::Grok)
        && let Ok(s) = std::fs::read_to_string(home.join("version.json"))
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        && let Some(ver) = v.get("version").and_then(|x| x.as_str())
        && !ver.is_empty()
        && ver.len() <= 64
    {
        return Some(ver.to_string());
    }
    // gemini-cli records its version in the installed package manifest.
    if matches!(source, PeerSource::Gemini)
        && let Ok(s) = std::fs::read_to_string(home.join("package.json"))
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        && let Some(ver) = v.get("version").and_then(|x| x.as_str())
        && !ver.is_empty()
        && ver.len() <= 64
    {
        return Some(ver.to_string());
    }
    // Both original peers also record a version inside their manifest, when present.
    if let Ok(s) = std::fs::read_to_string(home.join("MANIFEST.json"))
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        && let Some(ver) = v.get("source_version").and_then(|x| x.as_str())
        && !ver.is_empty()
        && ver.len() <= 64
    {
        return Some(ver.to_string());
    }
    None
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

    // F26-GRADE-H1: this block used to list skills, personas and memory as
    // "Detected but NOT imported" in the SAME run whose accounting counted them
    // as imported — two incompatible statements about one set of items. Those
    // three classes are imported now, so they are no longer listed here.
    // `deferred_other` remains, because the classes it names genuinely are not
    // imported and naming them is what keeps them from being silently lost.
    if !plan.deferred_other.is_empty() {
        println!("\nDetected but NOT imported — no Wayland equivalent to map onto:");
        for (kind, n) in &plan.deferred_other {
            println!("  • {n} {kind}");
        }
        println!(
            "  These stay on the source install. Importing them would mean guessing a\n  \
             mapping, and a wrong guess in a settings key changes runtime behaviour."
        );
    }
    for w in &plan.warnings {
        println!("  ! {w}");
    }
}

/// The containment half of the PLAN preview.
///
/// Stated in the dry run as well as the apply, because a containment the user
/// only learns about afterwards is discovered later as missing functionality
/// and worked around.
fn render_containment(published: &[PublishedItem], selection: &Selection) {
    let exec: Vec<&PublishedItem> = published
        .iter()
        .filter(|p| p.class == "executable" && selection.wants(&p.identity))
        .collect();
    if selection.is_narrowed() {
        let wanted = published
            .iter()
            .filter(|p| selection.wants(&p.identity))
            .count();
        println!(
            "\nSelection: {wanted} of {} discovered item{} ({} excluded).",
            published.len(),
            plural(published.len(), "", "s"),
            published.len() - wanted,
        );
    }
    if exec.is_empty() {
        return;
    }
    println!(
        "\nWill be QUARANTINED — imported but INERT until you promote them ({}):",
        exec.len()
    );
    for p in &exec {
        println!(
            "  • {} — {}",
            p.identity,
            p.executable_reason.as_deref().unwrap_or("executable")
        );
    }
    println!(
        "  Quarantined content is written outside every directory the agent loads skills from,\n  \
         so it cannot run. Review it, then promote what you want with:\n    \
         wayland-core migrate promote --id <identity>"
    );
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
    println!(
        "Accounting: discovered={} imported={} quarantined={} excluded={} (the last three sum to the first).",
        report.discovered, report.imported, report.quarantined, report.excluded,
    );
    // F26-GRADE-H1: the item counts above are now reported BESIDE the file
    // count they are supposed to describe. If content is claimed imported and
    // no files were written, the two lines contradict each other where a user
    // can see it — which is the whole difference between this report and the
    // one that said `imported=14` over four files.
    println!(
        "Content written: {} file{} — {} skill{}, {} persona{}, {} memory note{}.",
        report.files_written,
        plural(report.files_written, "", "s"),
        report.skills_imported,
        plural(report.skills_imported, "", "s"),
        report.personas_imported,
        plural(report.personas_imported, "", "s"),
        report.memory_imported,
        plural(report.memory_imported, "", "s"),
    );
    if report.skills_deduplicated > 0 {
        println!(
            "  ({} skill{} were byte-identical to one already imported and share its copy.)",
            report.skills_deduplicated,
            plural(report.skills_deduplicated, "", "s"),
        );
    }
    if report.exec_bits_stripped > 0 {
        println!(
            "  {} imported file{} carried an execute bit; it was REMOVED. Measured against the\n  \
             real peer trees, 68 of 349 peer skills ship a .sh/.py/.js helper, and a skill is\n  \
             classified on its SKILL.md prose — so those helpers import live. They arrive inert:\n  \
             running one is an explicit act (`sh <script>`), which goes through tool approval.",
            report.exec_bits_stripped,
            plural(report.exec_bits_stripped, "", "s"),
        );
    }
    if report.personas_imported > 0 || report.memory_imported > 0 {
        println!(
            "  Personas and memory notes are STAGED under {}/ — they are not active until\n  \
             you move them where you want them. Personas are stored with forged trust\n  \
             delimiters neutralized.",
            content::IMPORT_STAGE_DIR,
        );
    }
    if report.quarantined > 0 {
        println!(
            "\nQuarantined {} item{} — inert until an explicit promotion:",
            report.quarantined,
            plural(report.quarantined, "", "s"),
        );
        for n in &report.quarantine_notices {
            println!("  • {n}");
        }
        println!("  Review with: wayland-core migrate quarantined");
    }
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
            credential_file: None,
            mcp_refs: Vec::new(),
            conflict,
            source_path: format!("profiles/{name}"),
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
            deferred_other: BTreeMap::new(),
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
                allowed_tools: None,
            },
        );
        assert!(!plan.is_empty(false));
    }
}
