// `hello` is a framework-validation fixture (see `hello.rs`) — compiled and
// registered ONLY under `cfg(test)` so it never reaches the shipped skill
// catalog. In production it leaked: models saw it in every session's catalog
// and narrated skipping it into user-facing output.
#[cfg(test)]
mod hello;

use std::path::PathBuf;

use crate::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Definition for a bundled skill compiled into the binary.
///
/// All string fields use `&'static str` because bundled skill definitions are
/// compile-time constants embedded in the binary.
pub struct BundledSkillDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub when_to_use: Option<&'static str>,
    pub argument_hint: Option<&'static str>,
    pub allowed_tools: &'static [&'static str],
    pub model: Option<&'static str>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    /// "inline" | "fork"
    pub context: Option<&'static str>,
    pub agent: Option<&'static str>,
    /// Embedded reference files: (relative_path, content) pairs.
    /// Extracted to disk on first invocation via `extract_bundled_skill_files`.
    pub files: &'static [(&'static str, &'static str)],
    /// Skill body content (Markdown).
    pub content: &'static str,
}

/// Session-owned bundled skill data.
///
/// Embedded definitions are copied into this owned shape, while plugin
/// adapters can move their owned strings into it without leaking memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledSkillEntry {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub argument_hint: Option<String>,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    /// "inline" | "fork"
    pub context: Option<String>,
    pub agent: Option<String>,
    /// Embedded reference files: (relative_path, content) pairs.
    pub files: Vec<(String, String)>,
    /// Skill body content (Markdown).
    pub content: String,
}

impl From<BundledSkillDefinition> for BundledSkillEntry {
    fn from(def: BundledSkillDefinition) -> Self {
        Self {
            name: def.name.to_owned(),
            description: def.description.to_owned(),
            when_to_use: def.when_to_use.map(str::to_owned),
            argument_hint: def.argument_hint.map(str::to_owned),
            allowed_tools: def
                .allowed_tools
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect(),
            model: def.model.map(str::to_owned),
            disable_model_invocation: def.disable_model_invocation,
            user_invocable: def.user_invocable,
            context: def.context.map(str::to_owned),
            agent: def.agent.map(str::to_owned),
            files: def
                .files
                .iter()
                .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
                .collect(),
            content: def.content.to_owned(),
        }
    }
}

/// Bundled and plugin skills owned by one bootstrap/session.
///
/// Entries retain insertion order. `embedded()` installs embedded definitions
/// first; bootstrap then appends plugin entries so existing precedence stays
/// embedded-first, plugin-second.
#[derive(Debug, Default)]
pub struct BundledSkillCatalog {
    entries: Vec<BundledSkillEntry>,
}

impl BundledSkillCatalog {
    /// Create an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a catalog containing only definitions embedded in this binary.
    pub fn embedded() -> Self {
        let mut catalog = Self::new();
        #[cfg(test)]
        hello::register_hello_skill(&mut catalog);
        catalog
    }

    /// Append one owned entry to this catalog.
    pub fn register(&mut self, entry: BundledSkillEntry) {
        self.entries.push(entry);
    }

    /// Convert this catalog to runtime metadata without extracting files.
    pub fn get_bundled_skills(&self) -> Vec<SkillMetadata> {
        self.entries.iter().map(entry_to_metadata).collect()
    }

    /// Convert this catalog to runtime metadata and extract reference files.
    pub async fn prepare_bundled_skills(&self) -> Vec<SkillMetadata> {
        let mut skills = self.get_bundled_skills();

        for entry in self.entries.iter().filter(|entry| !entry.files.is_empty()) {
            let files: Vec<(&str, &str)> = entry
                .files
                .iter()
                .map(|(path, content)| (path.as_str(), content.as_str()))
                .collect();
            if let Some(dir) = extract_bundled_skill_files(&entry.name, &files).await
                && let Some(meta) = skills.iter_mut().find(|meta| meta.name == entry.name)
            {
                meta.skill_root = Some(dir.to_string_lossy().into_owned());
            }
        }

        skills
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Append one compile-time bundled definition to an explicit catalog.
pub fn register_bundled_skill(catalog: &mut BundledSkillCatalog, def: BundledSkillDefinition) {
    catalog.register(def.into());
}

/// Get embedded-only bundled skills as `SkillMetadata`.
///
/// Does NOT extract files to disk — `skill_root` is always `None` for skills
/// that have embedded files. Use `prepare_bundled_skills()` from an async
/// context to get metadata with `skill_root` populated.
pub fn get_bundled_skills() -> Vec<SkillMetadata> {
    BundledSkillCatalog::embedded().get_bundled_skills()
}

/// Async version: get bundled skills with file extraction.
///
/// For each skill that has embedded `files`, calls `extract_bundled_skill_files`
/// and sets `skill_root` to the extraction directory on success. File extraction
/// failure is non-fatal — `skill_root` remains `None` and the skill still works.
///
/// Called from `load_all_skills()` (async context). Not suitable for sync callers.
pub async fn prepare_bundled_skills() -> Vec<SkillMetadata> {
    BundledSkillCatalog::embedded()
        .prepare_bundled_skills()
        .await
}

/// Initialize all built-in bundled skills.
///
/// Returns a fresh catalog each time, so one bootstrap cannot observe entries
/// appended by another bootstrap.
pub fn init_bundled_skills() -> BundledSkillCatalog {
    // The only bundled skill today is the `hello` test fixture, which must NOT
    // ship in the production catalog (models notice it and narrate skipping
    // it). Register it solely under `cfg(test)` so the bundled-skill framework
    // stays exercised by TC-10.04 / TC-10.28 without leaking to users. In a
    // shipped build this returns an empty catalog — correct, since no
    // production bundled skills exist yet.
    BundledSkillCatalog::embedded()
}

/// Returns the extraction directory for a bundled skill's reference files.
///
/// Path: `$TMPDIR/wayland-core-bundled-skills-{pid}/{skill_name}`
/// Uses PID as a per-process nonce to prevent symlink pre-creation attacks.
pub fn get_bundled_skill_extract_dir(skill_name: &str) -> PathBuf {
    let pid = std::process::id();
    let tmp = std::env::temp_dir();
    tmp.join(format!("wayland-core-bundled-skills-{pid}"))
        .join(skill_name)
}

/// F-086: remove the per-process bundled-skill extraction root directory.
///
/// Called at graceful shutdown to clean up the `$TMPDIR/wayland-core-bundled-skills-{pid}/`
/// directory that `extract_bundled_skill_files` creates. Best-effort: failures
/// are silently ignored (the OS will eventually purge `$TMPDIR`).
///
/// Register this with an `atexit`-style hook or call from the CLI's shutdown
/// path to prevent temp-dir accumulation across restarts.
pub fn cleanup_bundled_skill_extract_dir() {
    let pid = std::process::id();
    let root = std::env::temp_dir().join(format!("wayland-core-bundled-skills-{pid}"));
    if root.is_dir() {
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Extract a bundled skill's reference files to disk.
///
/// Security properties:
/// - Directory created with mode 0o700 (owner-only).
/// - Files written with `create_new(true)` (O_CREAT|O_EXCL) to prevent
///   overwriting existing files.
/// - On Unix, O_NOFOLLOW is added via `OpenOptionsExt` to prevent symlink
///   attacks on the final path component.
/// - Relative paths validated: `..` components and absolute paths are rejected.
///
/// Returns the extraction directory on success, or `None` if extraction fails.
/// Failure is non-fatal — the skill continues to work without a `skill_root`.
pub async fn extract_bundled_skill_files(
    skill_name: &str,
    files: &[(&str, &str)],
) -> Option<PathBuf> {
    if files.is_empty() {
        return None;
    }

    let dir = get_bundled_skill_extract_dir(skill_name);

    match write_skill_files(&dir, files).await {
        Ok(()) => Some(dir),
        Err(e) => {
            // Non-fatal: log and degrade gracefully (skill runs without skill_root)
            eprintln!(
                "[wayland-core] failed to extract bundled skill '{}' to {}: {}",
                skill_name,
                dir.display(),
                e
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: conversion
// ---------------------------------------------------------------------------

fn entry_to_metadata(entry: &BundledSkillEntry) -> SkillMetadata {
    let execution_context = match entry.context.as_deref() {
        Some("fork") => ExecutionContext::Fork,
        _ => ExecutionContext::Inline,
    };

    let content_length = entry.content.len();

    SkillMetadata {
        name: entry.name.clone(),
        display_name: None,
        description: entry.description.clone(),
        has_user_specified_description: true,
        allowed_tools: entry.allowed_tools.clone(),
        argument_hint: entry.argument_hint.clone(),
        argument_names: Vec::new(),
        when_to_use: entry.when_to_use.clone(),
        version: None,
        model: entry.model.clone(),
        disable_model_invocation: entry.disable_model_invocation,
        user_invocable: entry.user_invocable,
        execution_context,
        agent: entry.agent.clone(),
        effort: None,
        shell: None,
        paths: Vec::new(),
        artifacts: Vec::new(),
        hooks_raw: None,
        source: SkillSource::Bundled,
        loaded_from: LoadedFrom::Bundled,
        content: entry.content.clone(),
        content_length,
        // skill_root is set later by extract_bundled_skill_files in load_all_skills
        skill_root: None,
        max_turns: None,
        max_tokens: None,
    }
}

// ---------------------------------------------------------------------------
// Internal: file extraction
// ---------------------------------------------------------------------------

async fn write_skill_files(dir: &std::path::Path, files: &[(&str, &str)]) -> std::io::Result<()> {
    use std::collections::HashMap;

    // Group files by parent directory to minimise mkdir calls.
    let mut by_parent: HashMap<PathBuf, Vec<(PathBuf, &str)>> = HashMap::new();
    for (rel_path, content) in files {
        let target = resolve_skill_file_path(dir, rel_path)?;
        let parent = target
            .parent()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
            })?
            .to_owned();
        by_parent.entry(parent).or_default().push((target, content));
    }

    // Create directories and write files.
    for (parent, entries) in by_parent {
        create_dir_secure(&parent).await?;
        for (path, content) in entries {
            safe_write_file(&path, content).await?;
        }
    }

    Ok(())
}

/// Create a directory (and all parents) with owner-only permissions.
/// Unix: 0o700 via DirBuilderExt. Windows: create then restrict via icacls
/// (remove inherited ACEs, grant current user Full Control only).
///
/// Audit W-3 fix (E2E-WINDOWS-ADDENDUM-2026-05-24 §2.2):
/// The previous `#[cfg(not(unix))]` branch used `create_dir_all()` with no
/// ACL restriction, leaving bundled skill directories world-readable on Windows.
async fn create_dir_secure(dir: &std::path::Path) -> std::io::Result<()> {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
        }
        #[cfg(windows)]
        {
            std::fs::create_dir_all(&dir)?;
            // Remove inherited ACEs and grant the current user Full Control
            // only. icacls is present on all Windows >= Vista.
            // /reset  — restore inherited ACEs first (clean slate)
            // /inheritance:r — remove inheritance
            // /grant:r "%USERNAME%:(OI)(CI)F" — owner Full Control, inheritable
            // Errors are logged but do not fail the install: the directory is
            // under %APPDATA% which is already user-scoped; ACL tightening is
            // defence-in-depth.
            let path_str = dir.to_string_lossy().to_string();
            let username = std::env::var("USERNAME").unwrap_or_else(|_| "%USERNAME%".to_string());
            let grant_arg = format!("{username}:(OI)(CI)F");
            let _ = std::process::Command::new("icacls")
                .args([&path_str, "/reset", "/q"])
                .output();
            let _ = std::process::Command::new("icacls")
                .args([&path_str, "/inheritance:r", "/grant:r", &grant_arg, "/q"])
                .output();
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            std::fs::create_dir_all(&dir)
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Write `content` to `path` using O_CREAT|O_EXCL (and O_NOFOLLOW on Unix).
/// Fails if the file already exists or if `path` is a symlink (Unix only).
async fn safe_write_file(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let file = open_secure(path).await?;
    let mut file = tokio::fs::File::from_std(file);
    use tokio::io::AsyncWriteExt;
    file.write_all(content.as_bytes()).await?;
    file.flush().await
}

/// Open a file for writing with O_CREAT|O_EXCL (+ O_NOFOLLOW on Unix, mode 0o600).
/// On Windows: exclusive create + post-create icacls ACL restriction.
///
/// Audit W-3 fix (E2E-WINDOWS-ADDENDUM-2026-05-24 §2.2):
/// The previous Windows branch opened with no mode restriction. Files now get
/// icacls ACL tightening after creation (same defence-in-depth as the dir).
async fn open_secure(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let path = path.to_owned();
    // Use spawn_blocking because OpenOptions with custom_flags is synchronous.
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                // O_NOFOLLOW: refuse to open if final path component is a symlink.
                // Belt-and-suspenders alongside O_EXCL (mirrors TS implementation).
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
        }
        #[cfg(windows)]
        {
            // Exclusive create — no O_NOFOLLOW equivalent on Windows.
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            // Tighten ACLs: remove inheritance, grant current user only.
            let path_str = path.to_string_lossy().to_string();
            let username = std::env::var("USERNAME").unwrap_or_else(|_| "%USERNAME%".to_string());
            let grant_arg = format!("{username}:F");
            let _ = std::process::Command::new("icacls")
                .args([&path_str, "/inheritance:r", "/grant:r", &grant_arg, "/q"])
                .output();
            Ok(file)
        }
        #[cfg(not(any(unix, windows)))]
        {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Validate and resolve a skill-relative path.
/// Rejects absolute paths and any path containing `..` components.
fn resolve_skill_file_path(base_dir: &std::path::Path, rel_path: &str) -> std::io::Result<PathBuf> {
    let normalized = std::path::Path::new(rel_path);

    if normalized.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("bundled skill file path must be relative: {rel_path}"),
        ));
    }

    for component in normalized.components() {
        use std::path::Component;
        if matches!(component, Component::ParentDir) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("bundled skill file path escapes skill dir: {rel_path}"),
            ));
        }
    }

    Ok(base_dir.join(normalized))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "bundled_tests.rs"]
mod tests;
