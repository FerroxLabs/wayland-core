mod hello;

use std::path::PathBuf;
use std::sync::OnceLock;

// Wave RB STABILITY — replaced `std::sync::Mutex` with
// `parking_lot::Mutex` so a panic while holding the bundled-skill
// registry lock does not poison it. The critical sections are short
// `push` / `iter` operations that cannot leave the registry in an
// invalid state.
use parking_lot::Mutex;

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

// ---------------------------------------------------------------------------
// Global registry
// ---------------------------------------------------------------------------

static REGISTRY: OnceLock<Mutex<Vec<BundledSkillDefinition>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<BundledSkillDefinition>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a bundled skill definition into the global registry.
pub fn register_bundled_skill(def: BundledSkillDefinition) {
    registry().lock().push(def);
}

/// Get all registered bundled skills as `SkillMetadata`.
///
/// Does NOT extract files to disk — `skill_root` is always `None` for skills
/// that have embedded files. Use `prepare_bundled_skills()` from an async
/// context to get metadata with `skill_root` populated.
pub fn get_bundled_skills() -> Vec<SkillMetadata> {
    registry()
        .lock()
        .iter()
        .map(definition_to_metadata)
        .collect()
}

/// Async version: get bundled skills with file extraction.
///
/// For each skill that has embedded `files`, calls `extract_bundled_skill_files`
/// and sets `skill_root` to the extraction directory on success. File extraction
/// failure is non-fatal — `skill_root` remains `None` and the skill still works.
///
/// Called from `load_all_skills()` (async context). Not suitable for sync callers.
pub async fn prepare_bundled_skills() -> Vec<SkillMetadata> {
    let mut skills = get_bundled_skills();

    // Collect (name, files) for skills that have embedded reference files.
    let defs_with_files: Vec<(String, Vec<(&'static str, &'static str)>)> = {
        let guard = registry().lock();
        guard
            .iter()
            .filter(|d| !d.files.is_empty())
            .map(|d| (d.name.to_owned(), d.files.to_vec()))
            .collect()
    };

    for (name, files) in defs_with_files {
        if let Some(dir) = extract_bundled_skill_files(&name, &files).await
            && let Some(meta) = skills.iter_mut().find(|m| m.name == name)
        {
            meta.skill_root = Some(dir.to_string_lossy().into_owned());
        }
    }

    skills
}

/// Initialize all built-in bundled skills.
///
/// Clears the registry first to guarantee idempotency — safe to call multiple
/// times (useful in tests).
pub fn init_bundled_skills() {
    clear_bundled_skills_inner();
    hello::register_hello_skill();
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

fn definition_to_metadata(def: &BundledSkillDefinition) -> SkillMetadata {
    let execution_context = match def.context {
        Some("fork") => ExecutionContext::Fork,
        _ => ExecutionContext::Inline,
    };

    let content_length = def.content.len();

    SkillMetadata {
        name: def.name.to_owned(),
        display_name: None,
        description: def.description.to_owned(),
        has_user_specified_description: true,
        allowed_tools: def.allowed_tools.iter().map(|s| s.to_string()).collect(),
        argument_hint: def.argument_hint.map(str::to_owned),
        argument_names: Vec::new(),
        when_to_use: def.when_to_use.map(str::to_owned),
        version: None,
        model: def.model.map(str::to_owned),
        disable_model_invocation: def.disable_model_invocation,
        user_invocable: def.user_invocable,
        execution_context,
        agent: def.agent.map(str::to_owned),
        effort: None,
        shell: None,
        paths: Vec::new(),
        artifacts: Vec::new(),
        hooks_raw: None,
        source: SkillSource::Bundled,
        loaded_from: LoadedFrom::Bundled,
        content: def.content.to_owned(),
        content_length,
        // skill_root is set later by extract_bundled_skill_files in load_all_skills
        skill_root: None,
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
// Test helpers (registry reset only — no test logic here)
// ---------------------------------------------------------------------------

fn clear_bundled_skills_inner() {
    registry().lock().clear();
}

/// Clear the bundled skill registry.
///
/// Exposed for test isolation. Production code should use `init_bundled_skills()`
/// which calls this internally.
#[cfg(test)]
pub fn clear_bundled_skills() {
    clear_bundled_skills_inner();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "bundled_tests.rs"]
mod tests;
