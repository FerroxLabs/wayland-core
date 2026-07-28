//! Classification and containment of imported executable content (F26-02).
//!
//! # The contract, and where it comes from
//!
//! Core already decided this exact question once. GHSA-8r7g, in
//! `wcore_config::hooks`, established for project-defined hooks that foreign
//! executable content is **inert by default** (`trust_project_hooks` defaults
//! to `false`) and — the half that actually holds — that the foreign artifact
//! **cannot grant itself trust**, because only the operator's GLOBAL config
//! value is consulted. Imported peer state is the same threat arriving by a
//! different route, so it gets the same answer rather than a second, weaker
//! one.
//!
//! Both halves are implemented here:
//!
//! 1. **Inert by PLACEMENT, not by a flag.** Quarantined content is written
//!    under [`QuarantineStore::root`], which is `<wayland config dir>/`[`QUARANTINE_DIR`].
//!    None of the four agent-facing skill roots (`wcore_skills::paths`:
//!    `user_skills_dir`, `wayland_home_skills_dirs`, `project_skills_dirs`,
//!    `additional_skills_dirs`) resolves there. Inertness is therefore a
//!    property of WHERE THE BYTES ARE, not of a boolean somebody can flip or
//!    forget to check.
//! 2. **Nothing the content carries can promote it.** [`QuarantineStore::promote`]
//!    takes identities from its CALLER and consults exactly two things: those
//!    identities and the store's own index, which the store wrote. It reads no
//!    field, key, manifest entry or filename out of the quarantined payload. A
//!    `SKILL.md` whose frontmatter says `trusted: true`, a sibling `PROMOTE`
//!    marker, and a `manifest.json` claiming `"promoted": true` are all inert
//!    data to this module.
//!
//! # Classification uses the detector the executor actually enforces
//!
//! An imported skill body is executable exactly when
//! `wcore_skills::shell::contains_shell_commands` says so — the same predicate
//! `wcore_skills::permissions` keys its decision off and the same syntax
//! `wcore_skills::executor` runs. A second pattern list here would drift from
//! the one that is actually enforced, and content classified safe would still
//! execute.
//!
//! An imported peer MCP definition carrying a launch command is executable
//! because `migrate::hermes` already routes such a definition into an
//! `McpServerConfig` with `TransportType::Stdio`, and a stdio MCP server is
//! launched as a child process. That door is open today; this module brings it
//! under the same policy as skills.
//!
//! Personas, memory notes, settings and assets are DATA. Treating everything as
//! dangerous trains an operator to promote everything without reading it, which
//! hollows the contract out — so the breadth is bounded in both directions and
//! measured, not judged.
//!
//! # The ceilings are mirrored, never loosened
//!
//! `wcore_config::workspace_trust` bounds its executable surface at 512 files,
//! 4 MiB per file and 32 MiB in total, and refuses a symlinked executable file
//! outright. Those are the numbers here, restated in [`MAX_QUARANTINE_FILES`],
//! [`MAX_QUARANTINE_FILE_BYTES`] and [`MAX_QUARANTINE_TOTAL_BYTES`] because the
//! originals are private to that module. A drift guard in
//! `tests/migrate_quarantine.rs` reads `workspace_trust.rs` and fails if the
//! two ever disagree.
//!
//! A real Hermes home carries **540 skill directories**, so the 512 count
//! ceiling is a live constraint here, not a theoretical one. Raising it to
//! admit the import is expressly forbidden: it would widen the executable
//! surface bound for every workspace-trust consumer to solve a migration
//! problem. The collision is recorded as a finding with a severity instead.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use wcore_config::config::{McpServerConfig, wayland_config_dir};
use wcore_skills::shell::contains_shell_commands;
use wcore_skills::types::LoadedFrom;

use super::provenance::{Provenance, item_digest, normalize_relative_path, tree_item_digest};

/// Directory, under the Wayland config dir, that quarantined imports land in.
///
/// Deliberately NOT `skills`, NOT `skills/auto`, and NOT under any
/// `.wayland-core/` directory — those are the four roots the agent-facing
/// enumeration walks.
pub const QUARANTINE_DIR: &str = "migrate-quarantine";

/// Name of the store's own index inside [`QUARANTINE_DIR`].
pub const QUARANTINE_INDEX: &str = "index.json";

/// Subdirectory each quarantined item's bytes are stored under, so the index
/// can never be confused with a payload.
pub const QUARANTINE_PAYLOADS: &str = "payloads";

/// Mirrors `workspace_trust::MAX_EXECUTABLE_FILES`.
pub const MAX_QUARANTINE_FILES: usize = 512;
/// Mirrors `workspace_trust::MAX_EXECUTABLE_FILE_BYTES`.
pub const MAX_QUARANTINE_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Mirrors `workspace_trust::MAX_EXECUTABLE_TOTAL_BYTES`.
pub const MAX_QUARANTINE_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// What made an imported item executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableReason {
    /// The body carries a shell directive the skills executor recognizes.
    SkillShellDirective,
    /// A peer MCP definition carrying a launch command — a stdio server is a
    /// child process, not a setting.
    McpLaunchCommand,
    /// A hook definition carrying a command — the surface GHSA-8r7g closed for
    /// project configs, reachable again by a different route.
    HookCommand,
}

impl ExecutableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutableReason::SkillShellDirective => "skill body carries a shell directive",
            ExecutableReason::McpLaunchCommand => "mcp definition carries a launch command",
            ExecutableReason::HookCommand => "hook definition carries a command",
        }
    }
}

impl std::fmt::Display for ExecutableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kinds of thing an import can carry that are DATA by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Persona,
    MemoryNote,
    Settings,
    Asset,
}

/// The classification of one imported item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    Data,
    Executable(ExecutableReason),
}

impl Classification {
    pub fn is_executable(self) -> bool {
        matches!(self, Classification::Executable(_))
    }

    pub fn reason(self) -> Option<ExecutableReason> {
        match self {
            Classification::Executable(r) => Some(r),
            Classification::Data => None,
        }
    }
}

/// Classify an imported skill body.
///
/// Delegates to `wcore_skills::shell::contains_shell_commands` — the SAME
/// predicate the executor and the permission checker use, including its
/// exemption for MCP-loaded content (whose directives the executor returns
/// unchanged rather than running).
pub fn classify_skill_body(content: &str, loaded_from: LoadedFrom) -> Classification {
    if contains_shell_commands(content, loaded_from) {
        Classification::Executable(ExecutableReason::SkillShellDirective)
    } else {
        Classification::Data
    }
}

/// Classify an imported peer MCP definition.
///
/// A definition carrying a launch command is executable regardless of the
/// declared transport: `command` is what gets spawned, and a transport field is
/// peer-controlled data that must not be able to talk the classifier out of a
/// containment decision.
pub fn classify_mcp_server(server: &McpServerConfig) -> Classification {
    match server.command.as_deref() {
        Some(cmd) if !cmd.trim().is_empty() => {
            Classification::Executable(ExecutableReason::McpLaunchCommand)
        }
        _ => Classification::Data,
    }
}

/// Classify an imported hook definition by its command.
pub fn classify_hook_command(command: &str) -> Classification {
    if command.trim().is_empty() {
        Classification::Data
    } else {
        Classification::Executable(ExecutableReason::HookCommand)
    }
}

/// Personas, memory notes, settings and assets are data and import without
/// ceremony. Present as a function rather than a comment so the breadth of the
/// contract is stated in code and can be measured.
pub fn classify_data_kind(_kind: DataKind) -> Classification {
    Classification::Data
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum QuarantineError {
    #[error("quarantine I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("quarantine index is invalid: {0}")]
    InvalidIndex(#[from] serde_json::Error),
    #[error("imported executable content contains a symlink: {0}")]
    ExecutableSymlink(PathBuf),
    #[error("imported executable file exceeds {MAX_QUARANTINE_FILE_BYTES} bytes: {0}")]
    FileTooLarge(PathBuf),
    #[error(
        "imported executable surface exceeds the quarantine limits (max {MAX_QUARANTINE_FILES} files, {MAX_QUARANTINE_TOTAL_BYTES} bytes total)"
    )]
    SurfaceTooLarge,
    #[error("no quarantined item has identity {0:?}")]
    UnknownIdentity(String),
    #[error("promotion target already exists: {0}")]
    PromotionTargetExists(PathBuf),
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// One contained item, as the store recorded it.
///
/// Every field here is written by the STORE. Nothing in this record is read out
/// of the quarantined payload, which is what makes it safe for the promotion
/// path to consult.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineEntry {
    /// The published item identity — the same key selection and provenance use.
    pub id: String,
    /// Why it was contained.
    pub reason: ExecutableReason,
    /// Where its bytes live, relative to the store root.
    pub stored_path: String,
    /// Where it came from.
    pub provenance: Provenance,
    /// The item's on-disk name if promoted (a skill directory name).
    pub promote_as: String,
}

/// One item that left containment, and the name it landed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedItem {
    pub id: String,
    /// The directory name it was written as. Differs from the item's own name
    /// only when that name was already taken.
    pub promoted_as: String,
    /// True when a collision forced a disambiguated name.
    pub renamed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuarantineIndexFile {
    #[serde(default = "index_schema")]
    schema: u32,
    #[serde(default)]
    entries: BTreeMap<String, QuarantineEntry>,
}

const fn index_schema() -> u32 {
    1
}

impl Default for QuarantineIndexFile {
    fn default() -> Self {
        Self {
            schema: index_schema(),
            entries: BTreeMap::new(),
        }
    }
}

/// What the caller asks the store to contain.
#[derive(Debug, Clone)]
pub struct QuarantineRequest {
    pub id: String,
    pub reason: ExecutableReason,
    /// Directory holding the item's bytes (a skill directory), or `None` for a
    /// definition-only item (an MCP or hook entry), whose `inline` body is
    /// stored instead.
    pub source_dir: Option<PathBuf>,
    /// Body for a definition-only item.
    pub inline: Option<(String, Vec<u8>)>,
    /// Peer tool name.
    pub source_tool: String,
    /// Version the peer declares, if any.
    pub source_version: Option<String>,
    /// Path relative to the source home.
    pub source_path: String,
    /// Directory name to use if the item is ever promoted.
    pub promote_as: String,
}

/// The containment boundary.
#[derive(Debug, Clone)]
pub struct QuarantineStore {
    root: PathBuf,
}

impl QuarantineStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store for the current home. Resolved through `wayland_config_dir`
    /// so `WAYLAND_HOME` is honoured exactly as every other Wayland-owned path
    /// is.
    pub fn for_current_home() -> Self {
        Self::new(wayland_config_dir().join(QUARANTINE_DIR))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(QUARANTINE_INDEX)
    }

    fn load_index(&self) -> Result<QuarantineIndexFile, QuarantineError> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(QuarantineIndexFile::default());
        }
        let raw = fs::read_to_string(&path)?;
        if raw.trim().is_empty() {
            return Ok(QuarantineIndexFile::default());
        }
        Ok(serde_json::from_str(&raw)?)
    }

    fn save_index(&self, file: &QuarantineIndexFile) -> Result<(), QuarantineError> {
        fs::create_dir_all(&self.root)?;
        let json = serde_json::to_string_pretty(file)?;
        fs::write(self.index_path(), json)?;
        Ok(())
    }

    /// Every contained item, in identity order.
    pub fn entries(&self) -> Result<Vec<QuarantineEntry>, QuarantineError> {
        Ok(self.load_index()?.entries.into_values().collect())
    }

    pub fn contains(&self, id: &str) -> Result<bool, QuarantineError> {
        Ok(self.load_index()?.entries.contains_key(id))
    }

    /// Contain one item: copy its bytes under the store root, record its
    /// provenance, and index it.
    ///
    /// Refuses a symlinked executable file, an oversized file, and a surface
    /// over the count or total-byte ceiling — mirroring
    /// `workspace_trust::fingerprint_workspace` rather than inventing looser
    /// limits.
    pub fn admit(&self, req: &QuarantineRequest) -> Result<QuarantineEntry, QuarantineError> {
        let mut index = self.load_index()?;

        let rel_store = format!("{QUARANTINE_PAYLOADS}/{}", storage_name(&req.id));
        let dest = self.root.join(&rel_store);

        let (digest, files, bytes) = match (&req.source_dir, &req.inline) {
            (Some(dir), _) => {
                let collected = collect_bounded(dir)?;
                enforce_surface(index.entries.len() + collected.len(), total_of(&collected))?;
                let digest = tree_item_digest(&collected);
                write_tree(&dest, &collected)?;
                let n = collected.len();
                let b = total_of(&collected);
                (digest, n, b)
            }
            (None, Some((name, body))) => {
                let len = body.len() as u64;
                if len > MAX_QUARANTINE_FILE_BYTES {
                    return Err(QuarantineError::FileTooLarge(dest.join(name)));
                }
                enforce_surface(index.entries.len() + 1, len)?;
                let digest = item_digest(&req.source_path, body);
                fs::create_dir_all(&dest)?;
                fs::write(dest.join(name), body)?;
                (digest, 1, len)
            }
            (None, None) => {
                fs::create_dir_all(&dest)?;
                (item_digest(&req.source_path, b""), 0, 0)
            }
        };
        let _ = (files, bytes);

        let entry = QuarantineEntry {
            id: req.id.clone(),
            reason: req.reason,
            stored_path: rel_store,
            provenance: Provenance::new(
                req.source_tool.clone(),
                req.source_version.clone(),
                &req.source_path,
                digest,
            ),
            promote_as: req.promote_as.clone(),
        };
        index.entries.insert(req.id.clone(), entry.clone());
        self.save_index(&index)?;
        Ok(entry)
    }

    /// Promote the named identities into `dest_root`, one directory each.
    ///
    /// # The no-self-trust half of the contract
    ///
    /// `ids` comes from the CALLER — in production, from the operator's
    /// `migrate promote --id …` command line. This function reads nothing out
    /// of the quarantined payload: not its frontmatter, not a marker file, not
    /// a manifest, not its filename. The only other thing it consults is the
    /// store's own index, which the store wrote in [`Self::admit`]. So there is
    /// no field an imported artifact can carry that reaches this decision,
    /// which is the property GHSA-8r7g requires and the property a `trusted:
    /// true` frontmatter key would otherwise defeat.
    ///
    /// Promotion of a whole set costs ONE invocation, so promoting a realistic
    /// subset does not cost one operator action per item.
    ///
    /// # Why a name collision must not abort the set
    ///
    /// A real peer install carries the SAME skill name under many profiles —
    /// measured on 26-01's structural corpus, 256 quarantined items shared just
    /// 46 distinct directory names. Aborting the whole promotion on the first
    /// collision forces the operator to promote one item at a time, which is
    /// precisely the cost that makes an operator route around containment
    /// altogether. So a collision is RESOLVED, not fatal: the item is promoted
    /// under a name disambiguated by a digest of its identity, and the mapping
    /// is returned so the caller can report it. Nothing is silently overwritten
    /// and nothing is silently dropped.
    pub fn promote(
        &self,
        ids: &[String],
        dest_root: &Path,
    ) -> Result<Vec<PromotedItem>, QuarantineError> {
        let mut index = self.load_index()?;
        // Validate every identity BEFORE moving anything, so a typo in a set
        // cannot leave half a promotion applied.
        for id in ids {
            if !index.entries.contains_key(id) {
                return Err(QuarantineError::UnknownIdentity(id.clone()));
            }
        }
        fs::create_dir_all(dest_root)?;
        let mut promoted = Vec::new();
        let mut taken: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for id in ids {
            let entry = index.entries.get(id).expect("validated above").clone();
            let from = self.root.join(&entry.stored_path);

            // Resolve a unique on-disk name. The plain name is preferred; on a
            // collision the identity's digest disambiguates, so two items never
            // land on one directory and neither is lost.
            let mut name = entry.promote_as.clone();
            let mut renamed = false;
            if name.is_empty() || taken.contains(&name) || dest_root.join(&name).exists() {
                let digest = item_digest("id", entry.id.as_bytes());
                let base = if name.is_empty() { "item" } else { &name };
                name = format!("{}-{}", base, &digest[..12]);
                renamed = true;
            }
            if taken.contains(&name) || dest_root.join(&name).exists() {
                // Two DISTINCT identities cannot produce one digest, so reaching
                // here means the disambiguated name was already on disk — a real
                // conflict the operator must resolve rather than one to paper over.
                return Err(QuarantineError::PromotionTargetExists(
                    dest_root.join(&name),
                ));
            }

            copy_tree(&from, &dest_root.join(&name))?;
            fs::remove_dir_all(&from).ok();
            index.entries.remove(id);
            taken.insert(name.clone());
            promoted.push(PromotedItem {
                id: entry.id,
                promoted_as: name,
                renamed,
            });
        }
        self.save_index(&index)?;
        Ok(promoted)
    }
}

// ---------------------------------------------------------------------------
// Bounded collection — the mirrored refusals
// ---------------------------------------------------------------------------

fn total_of(files: &BTreeMap<String, Vec<u8>>) -> u64 {
    files.values().map(|b| b.len() as u64).sum()
}

fn enforce_surface(count: usize, total: u64) -> Result<(), QuarantineError> {
    if count > MAX_QUARANTINE_FILES || total > MAX_QUARANTINE_TOTAL_BYTES {
        return Err(QuarantineError::SurfaceTooLarge);
    }
    Ok(())
}

/// Read a source directory into memory, applying the mirrored refusals.
///
/// `symlink_metadata` is used so a symlink is seen AS a symlink rather than as
/// its target — the same choice `fingerprint_workspace` and
/// `profile::copy_tree_inner` make, and the reason a hostile link cannot
/// redirect the read out of the tree. Here it is a hard REFUSAL rather than a
/// silent skip, because a skipped link in an executable surface is a hole in
/// the digest.
fn collect_bounded(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, QuarantineError> {
    let mut out = BTreeMap::new();
    let mut total: u64 = 0;
    collect_inner(dir, dir, &mut out, &mut total)?;
    Ok(out)
}

fn collect_inner(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
    total: &mut u64,
) -> Result<(), QuarantineError> {
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Err(QuarantineError::ExecutableSymlink(dir.to_path_buf()));
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for path in entries {
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(QuarantineError::ExecutableSymlink(path));
        }
        if meta.is_dir() {
            collect_inner(root, &path, out, total)?;
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if meta.len() > MAX_QUARANTINE_FILE_BYTES {
            return Err(QuarantineError::FileTooLarge(path));
        }
        *total = total
            .checked_add(meta.len())
            .ok_or(QuarantineError::SurfaceTooLarge)?;
        if *total > MAX_QUARANTINE_TOTAL_BYTES || out.len() + 1 > MAX_QUARANTINE_FILES {
            return Err(QuarantineError::SurfaceTooLarge);
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        out.insert(normalize_relative_path(&rel), fs::read(&path)?);
    }
    Ok(())
}

fn write_tree(dest: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), QuarantineError> {
    fs::create_dir_all(dest)?;
    for (rel, bytes) in files {
        let target = dest.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, bytes)?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), QuarantineError> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(QuarantineError::ExecutableSymlink(path));
        }
        let target = to.join(entry.file_name());
        if meta.is_dir() {
            copy_tree(&path, &target)?;
        } else if meta.is_file() {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

/// A filesystem-safe storage name for an identity.
///
/// The identity is not used verbatim because it can contain `/` (see
/// `portability::ROOT_PROFILE_ID`) and other characters a path cannot carry. A
/// short digest suffix keeps two distinct identities from collapsing onto one
/// directory after sanitization.
fn storage_name(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let digest = item_digest("id", id.as_bytes());
    format!("{}-{}", &sanitized, &digest[..12])
}

// ---------------------------------------------------------------------------
// Scanning a peer home for executable content
// ---------------------------------------------------------------------------

/// One executable thing found in a peer home.
#[derive(Debug, Clone)]
pub struct ScannedExecutable {
    pub id: String,
    pub reason: ExecutableReason,
    pub dir: PathBuf,
    pub relative: String,
    pub name: String,
}

/// Directories a peer home keeps skills in, relative to the home.
///
/// Hermes keeps them at `skills/` and `profiles/<name>/skills/`; OpenClaw at
/// `plugin-skills/` and `agents/<name>/skills/`. Both roots are scanned for
/// either source, because a home carrying both layouts is a real migration and
/// guessing wrong loses items.
pub fn peer_skill_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![home.join("skills"), home.join("plugin-skills")];
    for parent in ["profiles", "agents"] {
        let base = home.join(parent);
        if let Ok(rd) = fs::read_dir(&base) {
            let mut kids: Vec<PathBuf> = rd
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            kids.sort();
            for k in kids {
                roots.push(k.join("skills"));
            }
        }
    }
    roots.retain(|p| p.is_dir());
    roots
}

/// Walk a peer home and classify every skill directory it carries.
///
/// A directory is a skill when it holds a `SKILL.md`. Its body is classified by
/// the existing detector; a body with no directive is DATA and importable
/// without promotion.
pub fn scan_peer_skills(home: &Path) -> Vec<(ScannedExecutable, Classification)> {
    let mut out = Vec::new();
    for root in peer_skill_roots(home) {
        let Ok(rd) = fs::read_dir(&root) else {
            continue;
        };
        let mut kids: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        kids.sort();
        for dir in kids {
            let skill_md = dir.join("SKILL.md");
            let Ok(body) = fs::read_to_string(&skill_md) else {
                continue;
            };
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let relative =
                normalize_relative_path(&dir.strip_prefix(home).unwrap_or(&dir).to_string_lossy());
            let class = classify_skill_body(&body, LoadedFrom::Skills);
            out.push((
                ScannedExecutable {
                    id: format!("skill:{relative}"),
                    reason: ExecutableReason::SkillShellDirective,
                    dir,
                    relative,
                    name,
                },
                class,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_config::config::TransportType;

    fn mcp(command: Option<&str>) -> McpServerConfig {
        McpServerConfig {
            transport: TransportType::Stdio,
            command: command.map(str::to_string),
            args: None,
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
            only_for_assistant: None,
        }
    }

    #[test]
    fn skill_classification_delegates_to_the_enforced_detector() {
        let exec = "---\nname: x\n---\nrun this\n\n```!\ntouch /tmp/x\n```\n";
        let inert = "---\nname: x\n---\njust prose, no directive\n";
        assert_eq!(
            classify_skill_body(exec, LoadedFrom::Skills),
            Classification::Executable(ExecutableReason::SkillShellDirective)
        );
        assert_eq!(
            classify_skill_body(inert, LoadedFrom::Skills),
            Classification::Data
        );
        // The detector's MCP exemption is respected rather than re-decided:
        // the executor returns MCP bodies unchanged, so they are not a shell
        // surface and must not be quarantined as one.
        assert_eq!(
            classify_skill_body(exec, LoadedFrom::Mcp),
            Classification::Data
        );
        // And the classifier agrees with the predicate the permission checker
        // uses, on the same inputs — the anti-drift claim, asserted.
        for (body, from) in [
            (exec, LoadedFrom::Skills),
            (inert, LoadedFrom::Skills),
            (exec, LoadedFrom::Mcp),
        ] {
            assert_eq!(
                classify_skill_body(body, from).is_executable(),
                contains_shell_commands(body, from)
            );
        }
    }

    #[test]
    fn an_mcp_launch_command_is_executable_and_a_urlonly_server_is_not() {
        assert_eq!(
            classify_mcp_server(&mcp(Some("node"))),
            Classification::Executable(ExecutableReason::McpLaunchCommand)
        );
        assert_eq!(classify_mcp_server(&mcp(None)), Classification::Data);
        assert_eq!(classify_mcp_server(&mcp(Some("   "))), Classification::Data);
    }

    #[test]
    fn a_hook_command_is_executable_and_data_kinds_are_not() {
        assert_eq!(
            classify_hook_command("./run.sh"),
            Classification::Executable(ExecutableReason::HookCommand)
        );
        assert_eq!(classify_hook_command(""), Classification::Data);
        for k in [
            DataKind::Persona,
            DataKind::MemoryNote,
            DataKind::Settings,
            DataKind::Asset,
        ] {
            assert_eq!(classify_data_kind(k), Classification::Data);
        }
    }

    #[test]
    fn storage_name_is_path_safe_and_collision_resistant() {
        let a = storage_name("skill:profiles/a/skills/x");
        assert!(!a.contains('/') && !a.contains(':'), "{a}");
        // Two identities that sanitize to the same string keep distinct names.
        assert_ne!(storage_name("a/b"), storage_name("a:b"));
    }

    #[test]
    fn the_quarantine_root_is_not_any_agent_facing_skill_root() {
        // Placement is the containment mechanism, so this is the load-bearing
        // structural claim: the store root is not the user skills dir, not a
        // WAYLAND_HOME skills dir, and not a `.wayland-core/skills` dir.
        let store = QuarantineStore::new(Path::new("/tmp/home").join(QUARANTINE_DIR));
        let root = store.root().to_path_buf();
        assert!(!root.ends_with("skills"), "{root:?}");
        assert!(!root.ends_with("auto"), "{root:?}");
        assert!(
            !root.to_string_lossy().contains(".wayland-core"),
            "{root:?}"
        );
        assert_eq!(root.file_name().unwrap(), QUARANTINE_DIR);
    }
}
