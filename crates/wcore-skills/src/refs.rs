//! X1: progressive skill loading.
//!
//! `SkillRef` is the lightweight view injected into the system prompt — name,
//! description, when_to_use, paths conditional, source. `SkillCatalog` is
//! the in-memory directory of refs; resolving a ref to a full `SkillMetadata`
//! reads the body off disk on demand.
//!
//! Compared to the prior eager `Vec<SkillMetadata>` where every body sat in
//! memory for the session, the catalog shape is bounded by the count of
//! refs (not by the sum of body sizes). The system prompt listing already
//! truncated descriptions to `MAX_LISTING_DESC_CHARS` (=250) per row, so
//! shrinking the in-memory shape has no token-cost regression — only a
//! resident-memory improvement.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::types::{LoadedFrom, SkillMetadata, SkillSource};

#[derive(Debug, Clone)]
pub struct SkillRef {
    pub name: String,
    pub display_name: Option<String>,
    pub description: String,
    pub when_to_use: Option<String>,
    pub paths: Vec<String>,
    pub source: SkillSource,
    pub loaded_from: LoadedFrom,
    /// Canonical filesystem path of the SKILL.md (or .md) file. Used by
    /// `resolve()` to read the body on first activation.
    pub file_path: PathBuf,
    /// Root directory for bundled reference files. Kept separately because
    /// bundled bodies resolve from `inline_content`, not from `file_path`.
    pub skill_root: Option<PathBuf>,
    /// Approximate body byte count from the loader pass. Used by audit
    /// to flag huge skills; never used for budgeting.
    pub content_length_hint: usize,
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
    /// Hint that the underlying skill declares `artifacts:` in frontmatter.
    /// Populated by `metadata_to_ref`; the audit uses this to skip loading
    /// bodies for skills that cannot have broken artifact refs.
    pub has_artifacts: bool,
    /// F-037: inline body content for bundled/plugin skills that have no
    /// real filesystem path. When `Some`, `resolve()` uses this directly
    /// and never attempts a disk read. `None` for filesystem skills.
    pub inline_content: Option<String>,
}

pub struct SkillCatalog {
    refs: Vec<SkillRef>,
    /// LRU-bounded cache of resolved bodies. Bounded so a runaway sequence
    /// of activations can't grow the session resident set unboundedly.
    /// 32 is a heuristic — typical sessions touch <10 distinct skills.
    cache: Arc<Mutex<lru::LruCache<String, Arc<SkillMetadata>>>>,
    /// Eager map of names → metadata. Populated by `from_metadata_vec` only.
    /// When present, `resolve()` short-circuits on a hit rather than reading
    /// the file. This is the transitional path until callers move to the
    /// lazy `load_catalog → resolve` flow end-to-end.
    eager: std::collections::HashMap<String, Arc<SkillMetadata>>,
    /// W6 — root directory whose one-level subdirectories are treated as
    /// sibling projects. When set, a `resolve()` miss widens the lookup
    /// across sibling projects' `.wayland-core/skills/` directories before
    /// returning `NotFound`. `None` keeps single-project behaviour.
    cross_project_root: Option<PathBuf>,
}

impl SkillCatalog {
    pub fn from_refs(refs: Vec<SkillRef>) -> Self {
        Self {
            refs,
            cache: Arc::new(Mutex::new(lru::LruCache::new(
                // SAFETY: 32 is a non-zero compile-time constant.
                std::num::NonZeroUsize::new(32).expect("32 is non-zero"),
            ))),
            eager: std::collections::HashMap::new(),
            cross_project_root: None,
        }
    }

    /// W6 — opt into cross-project skill resolution. `root` is the directory
    /// whose immediate subdirectories are scanned as sibling projects (the
    /// same shape `wcore_memory::cross_project::discover_projects` expects:
    /// a sibling counts only if it carries a `memory.db`). When a
    /// `resolve()` call misses the local catalog, each sibling's
    /// `.wayland-core/skills/` directory is searched for the named skill.
    ///
    /// Discovery is best-effort: a missing root, no siblings, or a sibling
    /// without the skill all degrade silently to single-project `NotFound`.
    pub fn with_cross_project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.cross_project_root = Some(root.into());
        self
    }

    /// Build a catalog from a pre-loaded `Vec<SkillMetadata>` and seed an
    /// in-memory eager-map so subsequent `resolve()` calls return the same
    /// metadata without any disk read.
    ///
    /// Used by `bootstrap.rs` and tests where the eager `load_all_skills`
    /// path is still the simplest fixture. Once Task 5 swaps bootstrap to
    /// `load_catalog`, only tests should reach this helper.
    pub fn from_metadata_vec(metas: Vec<SkillMetadata>) -> Self {
        let refs: Vec<SkillRef> = metas.iter().map(metadata_to_ref).collect();
        let mut cat = Self::from_refs(refs);
        cat.eager = metas
            .into_iter()
            .map(|m| (m.name.clone(), Arc::new(m)))
            .collect();
        cat
    }

    pub fn len(&self) -> usize {
        self.refs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }

    /// Iterate every ref, including those hidden from the model.
    pub fn refs(&self) -> impl Iterator<Item = &SkillRef> {
        self.refs.iter()
    }

    /// Iterate refs visible to the model (`!disable_model_invocation`).
    pub fn visible(&self) -> impl Iterator<Item = &SkillRef> {
        self.refs.iter().filter(|r| !r.disable_model_invocation)
    }

    pub fn find(&self, name: &str) -> Option<&SkillRef> {
        let name = name.trim_start_matches('/');
        self.refs.iter().find(|r| r.name == name)
    }

    /// Names visible to the model — used by SkillTool to render
    /// "available skills" in error messages.
    pub fn visible_names(&self) -> Vec<String> {
        self.visible().map(|r| r.name.clone()).collect()
    }

    /// M3.6 — iterate every ref's name, including those hidden from the
    /// model. Used by the session-start `SkillPrioritizer` to surface the
    /// names it must reorder.
    pub fn iter_names(&self) -> impl Iterator<Item = String> + '_ {
        self.refs.iter().map(|r| r.name.clone())
    }

    /// M3.6 — reorder the catalog in place so the names in `priority_order`
    /// come first (in the order given). Names in `priority_order` that
    /// don't exist in the catalog are silently ignored. Refs whose names
    /// don't appear in `priority_order` keep their original relative order
    /// after the prioritized prefix.
    ///
    /// Stable for the unlisted suffix — important so the bootstrap-time
    /// reorder is a pure permutation when no telemetry exists.
    pub fn reorder_by(&mut self, priority_order: &[String]) {
        let mut ordered: Vec<SkillRef> = Vec::with_capacity(self.refs.len());
        for name in priority_order {
            if let Some(pos) = self.refs.iter().position(|r| &r.name == name) {
                ordered.push(self.refs.remove(pos));
            }
        }
        ordered.append(&mut self.refs);
        self.refs = ordered;
    }

    /// Synchronous metadata lookup. Returns Some only when the catalog
    /// was built via `from_metadata_vec` (which seeds the eager map)
    /// or when a prior `resolve()` populated the LRU. Used by
    /// `SkillTool::context_modifier_for` / `skill_hooks_for` which must
    /// remain non-async to satisfy the existing `Tool` trait surface.
    ///
    /// For lazy catalogs built via `load_catalog`, callers must await
    /// `resolve()` before they can read metadata fields.
    pub fn find_metadata_sync(&self, name: &str) -> Option<Arc<SkillMetadata>> {
        let normalized = name.trim_start_matches('/');
        if let Some(hit) = self.eager.get(normalized) {
            return Some(Arc::clone(hit));
        }
        // LRU is locked async; we can try a best-effort blocking_lock on
        // the current runtime. If it fails (no current runtime, or being
        // held), return None — caller falls back to resolve() if it can.
        let cache = Arc::clone(&self.cache);
        let normalized = normalized.to_string();
        match cache.try_lock() {
            Ok(mut guard) => guard.get(&normalized).map(Arc::clone),
            Err(_) => None,
        }
    }

    /// Resolve a ref to a fully-loaded SkillMetadata. Reads body from disk
    /// on first call; subsequent calls hit the LRU cache. If the catalog
    /// was built via `from_metadata_vec`, hits its eager map first and
    /// skips disk entirely.
    pub async fn resolve(&self, name: &str) -> Result<Arc<SkillMetadata>, ResolveError> {
        let normalized = name.trim_start_matches('/');

        // Eager-map hit (transitional `from_metadata_vec` path).
        if let Some(hit) = self.eager.get(normalized) {
            return Ok(Arc::clone(hit));
        }

        // Fast path: cache hit.
        {
            let mut cache = self.cache.lock().await;
            if let Some(hit) = cache.get(normalized) {
                return Ok(Arc::clone(hit));
            }
        }

        // Cache miss: look up the ref.
        let r = match self.refs.iter().find(|r| r.name == normalized) {
            Some(r) => r,
            None => {
                // W6 — not in the local catalog; widen the search to
                // sibling projects' skill directories. Best-effort: any
                // failure degrades to the original NotFound.
                if let Some(meta) = self.resolve_cross_project(normalized).await {
                    let arc = Arc::new(meta);
                    let mut cache = self.cache.lock().await;
                    cache.put(normalized.to_string(), Arc::clone(&arc));
                    return Ok(arc);
                }
                return Err(ResolveError::NotFound(normalized.to_string()));
            }
        };

        // F-037: bundled/plugin skills carry their body in `inline_content`
        // (populated by `metadata_to_ref` when `source == Bundled`). Use it
        // directly to avoid a disk read against the virtual `<virtual:name>`
        // path, which would always fail with "No such file or directory".
        let raw: String;
        let skill_root: Option<String>;
        if let Some(inline) = &r.inline_content {
            raw = inline.clone();
            skill_root = r
                .skill_root
                .as_deref()
                .and_then(|root| root.to_str())
                .map(str::to_owned);
        } else {
            // Read body off disk (async).
            let bytes = tokio::fs::read(&r.file_path)
                .await
                .map_err(|source| ResolveError::Io {
                    path: r.file_path.clone(),
                    source,
                })?;
            raw = String::from_utf8_lossy(&bytes).into_owned();
            skill_root = r
                .skill_root
                .as_deref()
                .or_else(|| r.file_path.parent())
                .and_then(|root| root.to_str())
                .map(str::to_owned);
        }

        // Parse frontmatter. The loader pass already validated this skill;
        // failing here means the file changed under us.
        let parsed = crate::frontmatter::parse_frontmatter_with_source(
            &raw,
            Some(&r.file_path.to_string_lossy()),
        );
        let metadata = crate::frontmatter::parse_skill_fields(
            &parsed.frontmatter,
            &parsed.content,
            &r.name,
            r.source,
            r.loaded_from,
            skill_root.as_deref(),
        );

        let arc = Arc::new(metadata);

        // Populate cache.
        {
            let mut cache = self.cache.lock().await;
            cache.put(normalized.to_string(), Arc::clone(&arc));
        }

        Ok(arc)
    }

    /// Resolve a skill on behalf of the model. Unlike [`Self::resolve`], this
    /// treats model-hidden skills as absent, including guessed names and
    /// cross-project matches. Operator-facing inspection paths may continue
    /// to use the unrestricted resolver.
    pub async fn resolve_for_model(&self, name: &str) -> Result<Arc<SkillMetadata>, ResolveError> {
        let normalized = name.trim_start_matches('/');

        if let Some(local) = self.refs.iter().find(|r| r.name == normalized) {
            if local.disable_model_invocation {
                return Err(ResolveError::NotFound(normalized.to_string()));
            }
            let resolved = self.resolve(normalized).await?;
            let generated = if local.inline_content.is_none() {
                match local.file_path.parent() {
                    Some(skill_dir) => {
                        crate::loader::is_generated_draft(
                            skill_dir,
                            &resolved.name,
                            &resolved.content,
                        )
                        .await
                    }
                    None => false,
                }
            } else {
                false
            };
            if resolved.disable_model_invocation || generated {
                return Err(ResolveError::NotFound(normalized.to_string()));
            }
            return Ok(resolved);
        }

        // An unrestricted operator lookup may already have cached a sibling
        // project skill. Re-check its resolved metadata before returning it to
        // the model rather than treating the cache as an authority boundary.
        {
            let mut cache = self.cache.lock().await;
            if let Some(hit) = cache.get(normalized) {
                if hit.disable_model_invocation {
                    return Err(ResolveError::NotFound(normalized.to_string()));
                }
                return Ok(Arc::clone(hit));
            }
        }

        let Some(meta) = self.resolve_cross_project(normalized).await else {
            return Err(ResolveError::NotFound(normalized.to_string()));
        };
        if meta.disable_model_invocation {
            return Err(ResolveError::NotFound(normalized.to_string()));
        }

        let arc = Arc::new(meta);
        let mut cache = self.cache.lock().await;
        cache.put(normalized.to_string(), Arc::clone(&arc));
        Ok(arc)
    }

    /// W6 — widen a resolution miss across sibling projects.
    ///
    /// Consults `wcore_memory::cross_project::discover_projects` against the
    /// configured `cross_project_root`, then scans each discovered sibling's
    /// `.wayland-core/skills/` directory for a skill named `name`. The first
    /// match wins. Returns `None` — never an error — when cross-project
    /// resolution is disabled, finds no siblings, or no sibling carries the
    /// skill, so the caller can fall through to single-project `NotFound`.
    async fn resolve_cross_project(&self, name: &str) -> Option<SkillMetadata> {
        let root = self.cross_project_root.as_deref()?;
        let projects = wcore_memory::cross_project::discover_projects(root);
        if projects.is_empty() {
            return None;
        }
        for project in projects {
            // `memory_db_path` is `<project_dir>/memory.db`; the project dir
            // is its parent. Sibling skills live under
            // `<project_dir>/.wayland-core/skills/`.
            let Some(project_dir) = project.memory_db_path.parent() else {
                continue;
            };
            let skills_dir = project_dir.join(".wayland-core").join("skills");
            if !skills_dir.is_dir() {
                continue;
            }
            let loaded = crate::loader::load_skills_from_dir(
                &skills_dir,
                SkillSource::Project,
                LoadedFrom::Skills,
            )
            .await;
            if let Some(hit) = loaded.into_iter().find(|s| s.metadata.name == name) {
                tracing::debug!(
                    skill = name,
                    project = %project.project_id,
                    "resolved skill from sibling project"
                );
                return Some(hit.metadata);
            }
        }
        None
    }
}

/// Project a `SkillMetadata` to a `SkillRef`. Shared between loader and
/// the `from_metadata_vec` test-helper path.
///
/// F-037: For bundled/plugin skills that have no real filesystem path (i.e.,
/// `skill_root` is `None` and `source == Bundled`), the skill body is stored
/// in `SkillMetadata.content` at registration time. We capture it into
/// `inline_content` so `resolve()` can return it directly without attempting
/// any disk read against the virtual `<virtual:name>` path.
pub fn metadata_to_ref(m: &SkillMetadata) -> SkillRef {
    let skill_root = m.skill_root.as_deref().map(std::path::PathBuf::from);
    let file_path = m
        .skill_root
        .as_deref()
        .map(|root| std::path::Path::new(root).join("SKILL.md"))
        .unwrap_or_else(|| std::path::PathBuf::from(format!("<virtual:{}>", m.name)));

    // Bundled skills (native + plugin-contributed) have their body embedded
    // in `content` at registration time. Capture it so resolve() can bypass
    // the disk read that would fail on the virtual path.
    let inline_content = if m.source == SkillSource::Bundled && !m.content.is_empty() {
        Some(m.content.clone())
    } else {
        None
    };

    SkillRef {
        name: m.name.clone(),
        display_name: m.display_name.clone(),
        description: m.description.clone(),
        when_to_use: m.when_to_use.clone(),
        paths: m.paths.clone(),
        source: m.source,
        loaded_from: m.loaded_from,
        file_path,
        skill_root,
        content_length_hint: m.content_length,
        user_invocable: m.user_invocable,
        disable_model_invocation: m.disable_model_invocation,
        has_artifacts: !m.artifacts.is_empty(),
        inline_content,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("skill not found in catalog: {0}")]
    NotFound(String),

    #[error("failed to read skill body at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse frontmatter at {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LoadedFrom, SkillSource};

    fn make_ref(name: &str, description: &str) -> SkillRef {
        SkillRef {
            name: name.to_string(),
            display_name: None,
            description: description.to_string(),
            when_to_use: None,
            paths: Vec::new(),
            source: SkillSource::Project,
            loaded_from: LoadedFrom::Skills,
            file_path: std::path::PathBuf::from(format!("/tmp/{name}/SKILL.md")),
            skill_root: None,
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation: false,
            has_artifacts: false,
            inline_content: None,
        }
    }

    #[test]
    fn skill_ref_carries_only_listing_fields() {
        let r = make_ref("greet", "Greet a user by name");
        assert_eq!(r.name, "greet");
        assert_eq!(r.description, "Greet a user by name");
        assert!(r.when_to_use.is_none());
    }

    #[test]
    fn catalog_lookup_by_name_is_case_sensitive_with_slash_strip() {
        let cat = SkillCatalog::from_refs(vec![make_ref("greet", "")]);
        assert!(cat.find("greet").is_some());
        assert!(
            cat.find("/greet").is_some(),
            "leading slash must be stripped"
        );
        assert!(cat.find("Greet").is_none(), "case-sensitive");
        assert!(cat.find("missing").is_none());
    }

    #[test]
    fn catalog_iter_yields_visible_refs_only() {
        let mut a = make_ref("a", "");
        a.disable_model_invocation = true;
        let b = make_ref("b", "");
        let cat = SkillCatalog::from_refs(vec![a, b]);
        let visible: Vec<&str> = cat.visible().map(|r| r.name.as_str()).collect();
        assert_eq!(
            visible,
            vec!["b"],
            "disable_model_invocation hides from catalog"
        );
    }

    #[tokio::test]
    async fn resolve_reads_body_off_disk_first_time() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("greet");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(
            &skill_path,
            "---\nname: greet\ndescription: Greet\n---\n\nHello, $ARGUMENTS!\n",
        )
        .unwrap();

        let mut r = make_ref("greet", "Greet");
        r.file_path = skill_path.clone();

        let cat = SkillCatalog::from_refs(vec![r]);
        let m = cat.resolve("greet").await.expect("resolve");

        assert_eq!(m.name, "greet");
        assert!(m.content.contains("Hello, $ARGUMENTS!"));
    }

    #[tokio::test]
    async fn resolve_unknown_name_returns_not_found() {
        let cat = SkillCatalog::from_refs(vec![]);
        match cat.resolve("nope").await {
            Err(ResolveError::NotFound(n)) => assert_eq!(n, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn model_resolver_rejects_hidden_eager_skill() {
        let mut hidden = make_ref("hidden", "generated");
        hidden.disable_model_invocation = true;
        let cat = SkillCatalog::from_refs(vec![hidden]);

        assert!(matches!(
            cat.resolve_for_model("hidden").await,
            Err(ResolveError::NotFound(name)) if name == "hidden"
        ));
    }

    #[tokio::test]
    async fn model_resolver_rechecks_lazily_loaded_local_visibility() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("late-hidden");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(
            &skill_path,
            "---\nname: late-hidden\ndescription: hidden\n\
             hide-from-slash-command-tool: true\n---\n\nSECRET BODY\n",
        )
        .unwrap();

        let mut stale_visible = make_ref("late-hidden", "stale visible listing");
        stale_visible.file_path = skill_path;
        let cat = SkillCatalog::from_refs(vec![stale_visible]);

        let operator = cat.resolve("late-hidden").await.expect("operator lookup");
        assert!(operator.disable_model_invocation);
        assert!(matches!(
            cat.resolve_for_model("late-hidden").await,
            Err(ResolveError::NotFound(name)) if name == "late-hidden"
        ));
    }

    #[tokio::test]
    async fn model_resolver_rechecks_lazy_local_generated_provenance() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("auto-late-generated");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(
            &skill_path,
            "---\nname: auto-late-generated\ndescription: generated\n---\n\nSECRET BODY\n",
        )
        .unwrap();
        std::fs::write(
            skill_dir.join("manifest.json"),
            r#"{"auto_drafted":true,"needs_review":false}"#,
        )
        .unwrap();

        let mut stale_visible = make_ref("auto-late-generated", "stale visible listing");
        stale_visible.file_path = skill_path;
        let cat = SkillCatalog::from_refs(vec![stale_visible]);

        assert!(cat.resolve("auto-late-generated").await.is_ok());
        assert!(matches!(
            cat.resolve_for_model("auto-late-generated").await,
            Err(ResolveError::NotFound(name)) if name == "auto-late-generated"
        ));
    }

    #[tokio::test]
    async fn resolve_io_error_surfaces_typed_error() {
        let mut r = make_ref("ghost", "");
        r.file_path = std::path::PathBuf::from("/nonexistent/ghost/SKILL.md");
        let cat = SkillCatalog::from_refs(vec![r]);
        match cat.resolve("ghost").await {
            Err(ResolveError::Io { path, .. }) => {
                assert!(path.to_string_lossy().contains("ghost"));
            }
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    /// W6 — write a minimal sibling project under `root` with one skill.
    /// Layout matches what `discover_projects` expects (`<proj>/memory.db`)
    /// plus a `.wayland-core/skills/<skill>/SKILL.md` body.
    fn make_sibling_project(root: &std::path::Path, project: &str, skill: &str) {
        let proj = root.join(project);
        std::fs::create_dir_all(&proj).unwrap();
        // discover_projects only counts a subdir that carries memory.db.
        std::fs::write(proj.join("memory.db"), b"").unwrap();
        let skill_dir = proj.join(".wayland-core").join("skills").join(skill);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {skill}\ndescription: from sibling\n---\n\nsibling body for {skill}\n"
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn resolve_falls_back_to_sibling_project() {
        // Sibling-projects root holds one other project carrying the skill.
        let root = tempfile::tempdir().unwrap();
        make_sibling_project(root.path(), "other-project", "shared");

        // Local catalog is empty; with cross-project root set, resolve()
        // must widen the search and find the sibling's skill.
        let cat =
            SkillCatalog::from_refs(vec![]).with_cross_project_root(root.path().to_path_buf());
        let m = cat
            .resolve("shared")
            .await
            .expect("skill resolves from sibling project");
        assert_eq!(m.name, "shared");
        assert!(m.content.contains("sibling body for shared"));
    }

    #[tokio::test]
    async fn model_resolver_rejects_generated_sibling_project_skill() {
        let root = tempfile::tempdir().unwrap();
        make_sibling_project(root.path(), "other-project", "auto-shared");
        let manifest = root
            .path()
            .join("other-project/.wayland-core/skills/auto-shared/manifest.json");
        std::fs::write(manifest, r#"{"auto_drafted":true,"needs_review":false}"#).unwrap();

        let cat =
            SkillCatalog::from_refs(vec![]).with_cross_project_root(root.path().to_path_buf());
        assert!(matches!(
            cat.resolve_for_model("auto-shared").await,
            Err(ResolveError::NotFound(name)) if name == "auto-shared"
        ));
        assert!(
            cat.resolve("auto-shared").await.is_ok(),
            "operator-facing resolution remains available for inspection"
        );
    }

    #[tokio::test]
    async fn resolve_degrades_to_not_found_when_no_siblings() {
        // Cross-project root is set but the directory has no sibling
        // projects — resolution must degrade to the normal NotFound, not
        // panic or error in cross-project code.
        let root = tempfile::tempdir().unwrap();
        let cat =
            SkillCatalog::from_refs(vec![]).with_cross_project_root(root.path().to_path_buf());
        match cat.resolve("missing").await {
            Err(ResolveError::NotFound(n)) => assert_eq!(n, "missing"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_without_cross_project_root_stays_single_project() {
        // No cross-project root configured: a miss is a plain NotFound and
        // sibling discovery is never consulted.
        let cat = SkillCatalog::from_refs(vec![]);
        match cat.resolve("anything").await {
            Err(ResolveError::NotFound(n)) => assert_eq!(n, "anything"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_caches_within_session() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("cached");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(
            &skill_path,
            "---\nname: cached\ndescription: ok\n---\n\nfirst content\n",
        )
        .unwrap();

        let mut r = make_ref("cached", "");
        r.file_path = skill_path.clone();
        let cat = SkillCatalog::from_refs(vec![r]);
        let first = cat.resolve("cached").await.unwrap();

        // Mutate file on disk; cached resolve should still return original.
        std::fs::write(
            &skill_path,
            "---\nname: cached\ndescription: ok\n---\n\nNEW content\n",
        )
        .unwrap();
        let second = cat.resolve("cached").await.unwrap();
        assert!(Arc::ptr_eq(&first, &second), "LRU must return same Arc");
        assert!(second.content.contains("first content"));
    }
}
