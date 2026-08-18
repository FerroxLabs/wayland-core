use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use futures::future::join_all;

use crate::bundled;
use crate::frontmatter::{parse_frontmatter_with_source, parse_skill_fields};
use crate::mcp::load_mcp_skills;
use crate::paths::{
    additional_skills_dirs, project_commands_dirs, project_skills_dirs, user_commands_dir,
    user_skills_dir, wayland_home_skills_dirs,
};
use crate::types::{LoadedFrom, SkillMetadata, SkillSource};
use wcore_mcp::manager::McpManager;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A loaded skill paired with its canonical filesystem path for deduplication.
pub struct LoadedSkill {
    pub metadata: SkillMetadata,
    /// Canonicalized path used for dedup (symlinks resolved, `.`/`..` removed).
    pub resolved_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load all skills from the filesystem and optionally from MCP servers.
///
/// Priority order (highest first): bundled → MCP → user → project → additional → legacy.
/// Deduplicates first by canonical path (symlinks resolved), then by name (first wins).
/// Bundled skills always take precedence over same-named MCP or filesystem skills.
///
/// If `bare` is true, only `add_dirs` are consulted (used for isolated
/// environments where the user/project directories should be ignored).
/// Bundled skills are included in bare mode as well.
///
/// Pass `mcp_manager: Some(&manager)` to include MCP-discovered skills.
pub async fn load_all_skills(
    cwd: &Path,
    add_dirs: &[PathBuf],
    bare: bool,
    mcp_manager: Option<&McpManager>,
) -> Vec<SkillMetadata> {
    let bundled_catalog = bundled::BundledSkillCatalog::embedded();
    load_all_skills_with_bundled(cwd, add_dirs, bare, mcp_manager, &bundled_catalog).await
}

/// Load all skills with a caller-owned bundled/plugin catalog.
///
/// Bootstrap uses this entry point so plugin entries remain local to the
/// session being constructed. Catalog insertion order is preserved before
/// the existing MCP and filesystem precedence rules are applied.
pub async fn load_all_skills_with_bundled(
    cwd: &Path,
    add_dirs: &[PathBuf],
    bare: bool,
    mcp_manager: Option<&McpManager>,
    bundled_catalog: &bundled::BundledSkillCatalog,
) -> Vec<SkillMetadata> {
    // Resolve bundled skills with file extraction (async context).
    let bundled_loaded = prepare_bundled_loaded(bundled_catalog).await;

    let mut all: Vec<LoadedSkill> = Vec::new();

    if bare {
        // Bare mode: only load from explicit add_dirs
        let dirs = additional_skills_dirs(add_dirs);
        let futures: Vec<_> = dirs
            .iter()
            .map(|d| load_skills_from_dir(d, SkillSource::Project, LoadedFrom::Skills))
            .collect();
        for batch in join_all(futures).await {
            all.extend(batch);
        }
        // Bundled skills prepended so they win deduplication
        all.splice(0..0, bundled_loaded);
        // Governance applies in bare mode too: an isolated environment is still one a
        // revoked skill must not execute in.
        return apply_governance(deduplicate_by_name(deduplicate(all))).await;
    }

    // 1. User-level skills (highest priority)
    if let Some(dir) = user_skills_dir()
        && dir.is_dir()
    {
        all.extend(load_skills_from_dir(&dir, SkillSource::User, LoadedFrom::Skills).await);
    }

    // 1b. `$WAYLAND_HOME`-rooted skills, including the auto-skill drafter's
    // `skills/auto/` write target. Treated as user-tier so auto-drafted
    // skills learned in a prior session load on the next boot. Read path ==
    // drafter write path (see `paths::wayland_home_skills_dirs`). The
    // recursive walk discovers each `<name>/SKILL.md`; later dedup-by-name
    // keeps the higher-priority `user_skills_dir()` copy when both resolve to
    // the same `$WAYLAND_HOME` and a skill appears in both.
    for dir in wayland_home_skills_dirs() {
        if dir.is_dir() {
            all.extend(load_skills_from_dir(&dir, SkillSource::User, LoadedFrom::Skills).await);
        }
    }

    // 2. Project-level skills (parallel across all dirs)
    let project_dirs = project_skills_dirs(cwd);
    let futures: Vec<_> = project_dirs
        .iter()
        .map(|d| load_skills_from_dir(d, SkillSource::Project, LoadedFrom::Skills))
        .collect();
    for batch in join_all(futures).await {
        all.extend(batch);
    }

    // 3. Additional dirs from --add-dir
    let add_skill_dirs = additional_skills_dirs(add_dirs);
    let futures: Vec<_> = add_skill_dirs
        .iter()
        .map(|d| load_skills_from_dir(d, SkillSource::Project, LoadedFrom::Skills))
        .collect();
    for batch in join_all(futures).await {
        all.extend(batch);
    }

    // 4. User-level legacy commands (lowest user priority)
    if let Some(dir) = user_commands_dir()
        && dir.is_dir()
    {
        all.extend(load_skills_from_commands_dir(&dir, SkillSource::User).await);
    }

    // 5. Project-level legacy commands (parallel)
    let cmd_dirs = project_commands_dirs(cwd);
    let futures: Vec<_> = cmd_dirs
        .iter()
        .map(|d| load_skills_from_commands_dir(d, SkillSource::Project))
        .collect();
    for batch in join_all(futures).await {
        all.extend(batch);
    }

    // MCP skills inserted after bundled (highest priority) but before filesystem
    // skills, so: bundled > MCP > user > project > additional > legacy.
    let mcp_loaded = match mcp_manager {
        Some(mgr) => load_mcp_skills(mgr).await,
        None => Vec::new(),
    };

    // Bundled skills first, then MCP, then filesystem
    all.splice(0..0, mcp_loaded);
    all.splice(0..0, bundled_loaded);

    // Path-based dedup first (handles symlinked duplicates), then name-based
    // dedup to enforce MCP vs. filesystem priority.
    apply_governance(deduplicate_by_name(deduplicate(all))).await
}

// ---------------------------------------------------------------------------
// 23A-C1: governance enforcement
// ---------------------------------------------------------------------------

/// Apply skill governance to a fully-resolved catalog.
///
/// Two effects, and the order between them is the resurrection fence:
///
/// 1. **A revoked skill is dropped from the catalog entirely.** Not quarantined —
///    *removed*. Quarantine only hides a skill from the model; the skill is still
///    loaded, still resolvable by name, and still executable through the user-invocable
///    path. A revocation that left the skill executable would not be a revocation.
/// 2. **A promoted skill has its generated-provenance quarantine lifted**, but only
///    while the bytes on disk still hash to the digest the grant names.
///
/// Revocation is checked **first and unconditionally**, so a stale promotion grant can
/// never re-expose a revoked artifact even if withdrawal did not complete.
///
/// This is the single choke point for both. It sits after dedup rather than inside
/// `load_skill_file` for two reasons: the governance state is read **once** per catalog
/// load instead of once per skill, and one placement is auditable where a dozen call
/// sites are not.
///
/// **Cost when nothing is governed is one directory read.** With no revocations and no
/// grants — the state of every install that has never used these commands — the function
/// returns the catalog untouched without hashing anything.
async fn apply_governance(skills: Vec<SkillMetadata>) -> Vec<SkillMetadata> {
    let Ok(store) = crate::govern::GovernanceStore::open_default() else {
        // No governance root resolvable on this platform. Governance is unavailable,
        // not violated; the catalog is unchanged.
        return skills;
    };
    let (Ok(revocations), Ok(grants)) = (store.live_revocations(), store.promotions()) else {
        // A governance read failed. Returning the catalog unchanged is the same
        // fail-open choice `is_revoked` documents, and for the same reason: failing
        // closed here would empty a user's whole skill catalog on one bad file.
        tracing::error!(
            target: "wcore_skills::loader",
            "could not read skill governance state; catalog left ungoverned"
        );
        return skills;
    };
    if revocations.is_empty() && grants.is_empty() {
        return skills;
    }

    let mut out = Vec::with_capacity(skills.len());
    for mut meta in skills {
        let Some(root) = meta.skill_root.clone() else {
            // Bundled and MCP skills have no directory. They are not drafted, cannot
            // be revoked by this surface, and are passed through.
            out.push(meta);
            continue;
        };
        let dir = PathBuf::from(&root);
        let dir_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let signature = crate::govern::read_signature(&dir);

        // ---- 1. revocation, first and unconditional ----
        let revoked = revocations.iter().any(|r| {
            r.skill_name == dir_name
                || match (r.signature.as_deref(), signature.as_deref()) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                }
        });
        if revoked {
            tracing::info!(
                target: "wcore_skills::loader",
                skill = %meta.name,
                path = %dir.display(),
                "skill is revoked; excluded from the catalog"
            );
            continue;
        }

        // ---- 2. promotion lifts quarantine, and only for generated drafts ----
        //
        // Gated on generated provenance so a promotion can never clear a
        // `disable-model-invocation` the *author* set in frontmatter. Promotion's
        // authority is over the quarantine this loader imposes, not over a user's
        // explicit declaration about their own skill.
        if meta.disable_model_invocation
            && grants.iter().any(|g| g.skill_name == dir_name)
            && is_generated_draft(&dir, &meta.name, &meta.content).await
        {
            let dir_for_check = dir.clone();
            let store_for_check = store.clone();
            let state = tokio::task::spawn_blocking(move || {
                store_for_check.promotion_state(&dir_for_check)
            })
            .await;
            match state {
                Ok(Ok(s)) if s.lifts_quarantine() => {
                    meta.disable_model_invocation = false;
                }
                Ok(Ok(crate::promote::PromotionState::DigestMismatch { promotion_id, .. })) => {
                    // Explicitly logged: a promoted skill silently reverting to
                    // quarantine after an edit is correct but surprising, and an
                    // unexplained reversion reads as a bug.
                    tracing::warn!(
                        target: "wcore_skills::loader",
                        skill = %meta.name,
                        promotion_id = %promotion_id,
                        "promotion grant does not match the bytes on disk; skill stays \
                         quarantined. Re-promote it to cover the current content."
                    );
                }
                _ => {}
            }
        }

        out.push(meta);
    }
    out
}

/// X1: load every skill's listing-only metadata without keeping bodies pinned.
///
/// Same discovery, dedup, and source-priority rules as `load_all_skills`, but
/// the returned `SkillRef`s carry only the fields the prompt listing needs
/// plus the file path on disk. `SkillCatalog::resolve()` reads bodies lazily.
///
/// This first cut reuses `load_all_skills` then drops the bodies — the
/// memory win against the eager path is realised once `Arc<SkillCatalog>`
/// replaces `Arc<Vec<SkillMetadata>>` at the agent layer. A future
/// micro-optimisation reads only frontmatter and skips the body allocation
/// on bootstrap; not required for W4's acceptance (prompt-cost is the
/// headline win, not steady-state RSS).
pub async fn load_catalog(
    cwd: &Path,
    add_dirs: &[PathBuf],
    bare: bool,
    mcp_manager: Option<&McpManager>,
) -> Vec<crate::refs::SkillRef> {
    let bundled_catalog = bundled::BundledSkillCatalog::embedded();
    load_catalog_with_bundled(cwd, add_dirs, bare, mcp_manager, &bundled_catalog).await
}

/// Load listing refs with a caller-owned bundled/plugin catalog.
pub async fn load_catalog_with_bundled(
    cwd: &Path,
    add_dirs: &[PathBuf],
    bare: bool,
    mcp_manager: Option<&McpManager>,
    bundled_catalog: &bundled::BundledSkillCatalog,
) -> Vec<crate::refs::SkillRef> {
    let full =
        load_all_skills_with_bundled(cwd, add_dirs, bare, mcp_manager, bundled_catalog).await;
    full.into_iter().map(metadata_to_ref).collect()
}

fn metadata_to_ref(m: SkillMetadata) -> crate::refs::SkillRef {
    crate::refs::metadata_to_ref(&m)
}

/// wayland#562 — load `skill://` resources from ONE MCP manager as listing
/// refs, for merging into an already-built catalog.
///
/// Used by the deferred config-MCP path (json-stream `defer_config_mcp`),
/// where the manager does not exist yet when `load_catalog` runs, so the
/// boot-time `mcp_manager: Some(..)` argument is `None` and these skills are
/// never seen. Runs the same dedup + `apply_governance` choke point the boot
/// catalog goes through, so a REVOKED skill cannot enter the session through
/// the late door that it is blocked from entering at boot.
pub async fn load_mcp_skill_refs(manager: &McpManager) -> Vec<crate::refs::SkillRef> {
    let loaded = load_mcp_skills(manager).await;
    if loaded.is_empty() {
        return Vec::new();
    }
    let governed = apply_governance(deduplicate(loaded)).await;
    governed.into_iter().map(metadata_to_ref).collect()
}

/// Lane D3 (G2/G4): load skills contributed by an installed marketplace plugin.
///
/// Scans `<plugin_skills_dir>/<name>/SKILL.md` (the bare `skills/` tree inside a
/// committed plugin dir) and namespaces each skill `<namespace>:<skill>` so a
/// plugin's skills never collide with user/project skills or with another
/// marketplace's. Returns listing refs ready to splice into the boot catalog;
/// bodies are read lazily, same as `load_catalog`.
pub async fn load_plugin_skill_catalog(
    plugin_skills_dir: &Path,
    namespace: &str,
) -> Vec<crate::refs::SkillRef> {
    let loaded =
        load_skills_from_dir(plugin_skills_dir, SkillSource::Project, LoadedFrom::Skills).await;
    loaded
        .into_iter()
        .map(|mut s| {
            s.metadata.name = format!("{namespace}:{}", s.metadata.name);
            metadata_to_ref(s.metadata)
        })
        .collect()
}

/// Prepare one caller-owned bundled catalog and wrap results as `LoadedSkill`.
///
/// Each bundled skill is assigned a virtual path `<bundled:name>` for
/// deduplication purposes (these paths can never match real filesystem paths).
async fn prepare_bundled_loaded(
    bundled_catalog: &bundled::BundledSkillCatalog,
) -> Vec<LoadedSkill> {
    bundled_catalog
        .prepare_bundled_skills()
        .await
        .into_iter()
        .map(|meta| {
            let virtual_path = PathBuf::from(format!("<bundled:{}>", meta.name));
            LoadedSkill {
                metadata: meta,
                resolved_path: virtual_path,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Internal: load from skills/ directory (directory-only format)
// ---------------------------------------------------------------------------

/// Load skills from a `skills/` directory.
///
/// Only the directory format is supported: each direct or nested subdirectory
/// that contains a `SKILL.md` file (case-sensitive) is loaded.
/// The skill name is derived from the relative path using colon separators.
pub(crate) async fn load_skills_from_dir(
    base_dir: &Path,
    source: SkillSource,
    loaded_from: LoadedFrom,
) -> Vec<LoadedSkill> {
    let mut results = Vec::new();
    collect_skill_md(base_dir, base_dir, source, loaded_from, &mut results).await;
    results
}

/// Recursively scan `dir` for `SKILL.md` files.
// This is a recursive async function — we use a Box::pin to satisfy the compiler.
fn collect_skill_md<'a>(
    base_dir: &'a Path,
    dir: &'a Path,
    source: SkillSource,
    loaded_from: LoadedFrom,
    results: &'a mut Vec<LoadedSkill>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let mut read_dir = match tokio::fs::read_dir(dir).await {
            Ok(rd) => rd,
            Err(_) => return,
        };

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();

            // F23A-C1-H4: never discover a promotion/rollback staging tree.
            //
            // `promote::staging_root_for` aims to put staging beside the skills root, and for
            // a flat `<root>/<name>` skill it does. For the layout the auto-drafter actually
            // writes — `skills/auto/auto-<sig>/` — the skill's parent is `skills/auto`, so
            // staging resolves to `skills/.promote-staging`, INSIDE this walk. A kill between
            // the copy and the `rename(2)` then leaves a half-built tree holding a `SKILL.md`
            // right where the loader will find it: the same "present, loadable, incomplete"
            // state F23A-C1-H3 removed from the target directory, arriving via the staging
            // directory instead.
            //
            // Fenced by name because the location cannot be guaranteed — `rename(2)` needs
            // staging on the target's filesystem, and skills roots nest arbitrarily through
            // `--add-dir`, `$WAYLAND_HOME` and project roots. Matched on this one directory
            // name rather than by a blanket dotted-directory rule, which would change which
            // skills load for users who never touched governance.
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n == crate::promote::STAGING)
            {
                continue;
            }

            // Follow symlinks: entry.file_type() does NOT traverse symlinks,
            // so use tokio::fs::metadata() which resolves the target type.
            let is_dir = match tokio::fs::metadata(&path).await {
                Ok(meta) => meta.is_dir(),
                Err(_) => continue,
            };

            if is_dir {
                // Check for SKILL.md directly inside this subdirectory using an
                // exact case-sensitive name comparison (important on case-insensitive
                // filesystems like macOS APFS).
                if let Some(skill_file) = find_exact_file(&path, "SKILL.md").await {
                    if let Some(skill) =
                        load_skill_file(&skill_file, base_dir, &path, source, loaded_from).await
                    {
                        results.push(skill);
                    }
                } else {
                    // Recurse into subdirectory (namespace nesting)
                    collect_skill_md(base_dir, &path, source, loaded_from, results).await;
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Internal: load from commands/ directory (legacy flat + directory format)
// ---------------------------------------------------------------------------

/// Load skills from a legacy `commands/` directory.
///
/// Supports two formats:
/// - Directory format: `<name>/SKILL.md` (takes precedence over flat `.md`)
/// - Flat format: `<name>.md` or `<subdir>/<name>.md`
async fn load_skills_from_commands_dir(base_dir: &Path, source: SkillSource) -> Vec<LoadedSkill> {
    let mut results = Vec::new();
    collect_commands(base_dir, base_dir, source, &mut results).await;
    results
}

fn collect_commands<'a>(
    base_dir: &'a Path,
    dir: &'a Path,
    source: SkillSource,
    results: &'a mut Vec<LoadedSkill>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let mut read_dir = match tokio::fs::read_dir(dir).await {
            Ok(rd) => rd,
            Err(_) => return,
        };

        // Collect all entries first so we can check for directory/flat conflicts
        let mut entries = Vec::new();
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            entries.push(entry);
        }

        // Track names that have a directory format (to skip their flat counterpart)
        let mut dir_names: HashSet<String> = HashSet::new();

        // First pass: handle directory format
        for entry in &entries {
            let path = entry.path();
            // Follow symlinks: use metadata() which resolves symlink targets.
            let is_dir = match tokio::fs::metadata(&path).await {
                Ok(meta) => meta.is_dir(),
                Err(_) => continue,
            };

            if is_dir {
                // Use exact case-sensitive lookup to avoid false positives on
                // case-insensitive filesystems (e.g., macOS APFS).
                if let Some(skill_file) = find_exact_file(&path, "SKILL.md").await {
                    // Directory format — load it
                    if let Some(skill) = load_skill_file(
                        &skill_file,
                        base_dir,
                        &path,
                        source,
                        LoadedFrom::CommandsDeprecated,
                    )
                    .await
                    {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        dir_names.insert(name);
                        results.push(skill);
                    }
                } else {
                    // Recurse: this is a namespace subdirectory (e.g., db/migrate.md)
                    collect_commands(base_dir, &path, source, results).await;
                }
            }
        }

        // Second pass: handle flat .md files (skip if directory version exists)
        for entry in &entries {
            let path = entry.path();
            // Follow symlinks: use metadata() to check if this is a file (not a dir symlink).
            let is_file = match tokio::fs::metadata(&path).await {
                Ok(meta) => meta.is_file(),
                Err(_) => continue,
            };

            if is_file && path.extension().and_then(|e| e.to_str()) == Some("md") {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();

                // Skip if a directory format was already loaded for this name
                if dir_names.contains(&stem) {
                    continue;
                }

                // The "skill directory" for flat files is their parent dir + stem
                let pseudo_dir = path.parent().unwrap_or(base_dir).join(&stem);
                if let Some(skill) = load_skill_file(
                    &path,
                    base_dir,
                    &pseudo_dir,
                    source,
                    LoadedFrom::CommandsDeprecated,
                )
                .await
                {
                    results.push(skill);
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Internal: load a single skill file
// ---------------------------------------------------------------------------

/// Read, parse, and return a `LoadedSkill` for a single Markdown file.
/// Returns `None` if the file cannot be read.
async fn load_skill_file(
    file_path: &Path,
    base_dir: &Path,
    skill_dir: &Path,
    source: SkillSource,
    loaded_from: LoadedFrom,
) -> Option<LoadedSkill> {
    let content = tokio::fs::read_to_string(file_path).await.ok()?;
    let parsed = parse_frontmatter_with_source(&content, Some(&file_path.to_string_lossy()));

    let resolved_name = build_namespace(base_dir, skill_dir);
    // skill_root is the directory containing SKILL.md (i.e., skill_dir itself),
    // used for ${WCORE_SKILL_DIR} (and the legacy ${AIONRS_SKILL_DIR} alias)
    // variable substitution in skill content.
    let skill_root = Some(skill_dir.to_string_lossy().into_owned());

    let mut metadata = parse_skill_fields(
        &parsed.frontmatter,
        &parsed.content,
        &resolved_name,
        source,
        loaded_from,
        skill_root.as_deref(),
    );

    // F06: generated provenance remains quarantined until F23 supplies a
    // governed promotion transaction. Review flags are not activation
    // authority. Keep drafts loaded for operator inspection, but never expose
    // them to model-facing catalog surfaces.
    if is_generated_draft(skill_dir, &resolved_name, &parsed.content).await {
        metadata.disable_model_invocation = true;
    }

    let resolved_path = try_canonicalize(file_path).unwrap_or_else(|| file_path.to_owned());

    Some(LoadedSkill {
        metadata,
        resolved_path,
    })
}

/// Generated provenance classifier for current and released drafts. A valid
/// `auto_drafted=true` manifest is authoritative regardless of review status.
/// Missing or damaged metadata falls back to the exact released body marker;
/// an `auto-*` name by itself never quarantines user-authored content.
pub(crate) async fn is_generated_draft(skill_dir: &Path, name: &str, content: &str) -> bool {
    let manifest = skill_dir.join("manifest.json");
    if let Ok(bytes) = tokio::fs::read(&manifest).await
        && serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| v.get("auto_drafted").and_then(serde_json::Value::as_bool))
            .unwrap_or(false)
    {
        return true;
    }
    crate::draft::is_released_generated_skill(name, content)
}

// ---------------------------------------------------------------------------
// Internal: namespace building
// ---------------------------------------------------------------------------

/// Build a colon-separated namespace from a directory hierarchy.
///
/// Examples:
/// - base=`<config_dir>/wayland-core/skills`, target=`<config_dir>/wayland-core/skills/db/migrate` → `"db:migrate"`
/// - base=`<config_dir>/wayland-core/skills`, target=`<config_dir>/wayland-core/skills/my-skill` → `"my-skill"`
pub(crate) fn build_namespace(base_dir: &Path, target_dir: &Path) -> String {
    match target_dir.strip_prefix(base_dir) {
        Ok(relative) => relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":"),
        Err(_) => target_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Internal: deduplication
// ---------------------------------------------------------------------------

/// Deduplicate loaded skills by canonical path. First occurrence wins.
fn deduplicate(skills: Vec<LoadedSkill>) -> Vec<SkillMetadata> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut result = Vec::new();

    for skill in skills {
        if seen.insert(skill.resolved_path) {
            result.push(skill.metadata);
        }
    }

    result
}

/// Deduplicate by skill name (case-sensitive). First occurrence wins.
///
/// Called after path-based dedup to enforce priority between bundled, MCP,
/// and filesystem skills that share the same name but have different paths.
fn deduplicate_by_name(skills: Vec<SkillMetadata>) -> Vec<SkillMetadata> {
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut result = Vec::new();

    for skill in skills {
        if seen.insert(skill.name.clone(), ()).is_none() {
            result.push(skill);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Internal: safe canonicalize
// ---------------------------------------------------------------------------

/// Canonicalize a path, returning `None` if the path does not exist.
/// Never panics.
pub(crate) fn try_canonicalize(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// Find a file with an exact case-sensitive name inside `dir`.
///
/// On case-insensitive filesystems (e.g., macOS APFS), `Path::is_file()` may
/// return `true` for `SKILL.md` even when only `skill.md` exists.  This
/// function reads the directory entries and performs a byte-for-byte name
/// comparison to avoid false positives.
///
/// Returns `None` if no entry with that exact name exists or if the directory
/// cannot be read.
async fn find_exact_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut rd = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        if entry.file_name().to_string_lossy() == name {
            let path = entry.path();
            let ft = entry.file_type().await.ok()?;
            if ft.is_file() {
                return Some(path);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "loader_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "loader_supplemental_tests.rs"]
mod supplemental_tests;
