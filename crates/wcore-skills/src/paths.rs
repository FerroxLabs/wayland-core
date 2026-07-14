use std::path::{Path, PathBuf};

use wcore_config::config::app_config_dir;

// ---------------------------------------------------------------------------
// User-level directories (<config_dir>/wayland-core/)
// ---------------------------------------------------------------------------

/// Return the user-level skills directory: `<config_dir>/wayland-core/skills/`
///
/// Returns `None` if the platform config directory cannot be determined.
pub fn user_skills_dir() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("skills"))
}

/// Return the user-level legacy commands directory: `<config_dir>/wayland-core/commands/`
pub fn user_commands_dir() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("commands"))
}

/// Return the `$WAYLAND_HOME`-rooted skill directories the auto-skill
/// `SkillDrafter` writes its drafts into, so the loader's read path matches
/// the drafter's write path exactly and auto-drafted skills become visible on
/// the next session.
///
/// Resolution mirrors `wcore_agent::bootstrap`'s drafter wiring precisely:
///   1. `$WAYLAND_HOME` (explicit env var), else
///   2. `~/.wayland` (home fallback).
///
/// Returns `[<root>/skills/auto, <root>/skills]`. The `auto/` subdir is the
/// drafter's legacy/secondary write target and is listed FIRST so the
/// recursive walk reaches each draft's `<name>/SKILL.md`. The parent `skills/`
/// dir is included second so manually-placed skills under
/// `$WAYLAND_HOME/skills/` are also picked up; when it overlaps
/// `user_skills_dir()` (the `WAYLAND_HOME` case) the loader's path-based dedup
/// collapses the duplicate. Returns an empty vec only when neither the env var
/// nor a home directory can be resolved (rare).
pub fn wayland_home_skills_dirs() -> Vec<PathBuf> {
    let root = match std::env::var("WAYLAND_HOME") {
        Ok(wh) => Some(PathBuf::from(wh)),
        Err(_) => dirs::home_dir().map(|h| h.join(".wayland")),
    };
    match root {
        Some(root) => {
            let skills = root.join("skills");
            let auto = skills.join("auto");
            vec![auto, skills]
        }
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Project-level directories (walk up from cwd)
// ---------------------------------------------------------------------------

/// Find all project-level `.wayland-core/skills/` directories by walking up from
/// `cwd` to the nearest git root (or home directory), returning deepest-first.
///
/// Deepest-first means the most-specific project directory wins in the
/// priority ordering (closer to cwd = higher priority).
pub fn project_skills_dirs(cwd: &Path) -> Vec<PathBuf> {
    walk_up_dirs(cwd, "skills")
}

/// Find all project-level `.wayland-core/commands/` directories (legacy), same walk.
pub fn project_commands_dirs(cwd: &Path) -> Vec<PathBuf> {
    walk_up_dirs(cwd, "commands")
}

/// Resolve additional skill directories from `--add-dir` paths.
///
/// Each path in `add_dirs` is checked for a `.wayland-core/skills/` subdirectory.
/// Only directories that exist are included.
pub fn additional_skills_dirs(add_dirs: &[PathBuf]) -> Vec<PathBuf> {
    add_dirs
        .iter()
        .map(|d| d.join(".wayland-core").join("skills"))
        .filter(|p| p.is_dir())
        .collect()
}

// ---------------------------------------------------------------------------
// Git root detection
// ---------------------------------------------------------------------------

/// Find the nearest git root from `start` by walking up looking for a `.git`
/// entry (file or directory). Returns `None` if no `.git` is found before
/// reaching the filesystem root or after `GIT_ROOT_DEPTH_CAP` ancestors.
///
/// F-087: capped at 32 ancestors to prevent slow walks on paths that are
/// deep inside a network FS or outside any git repository.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    wcore_config::workspace_trust::nearest_workspace_git_root(start)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Walk up from `cwd` to the git root (or home directory), collecting all
/// `.wayland-core/<subdir>/` directories that exist. Returns deepest-first.
fn walk_up_dirs(cwd: &Path, subdir: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for current in wcore_config::workspace_trust::executable_project_ancestors(cwd) {
        let candidate = current.join(".wayland-core").join(subdir);
        if candidate.is_dir() {
            dirs.push(candidate);
        }
    }

    dirs
}

/// Determine where to stop walking up. Stops at git root if found,
/// otherwise at the user home directory.
pub fn stop_boundary(cwd: &Path) -> Option<PathBuf> {
    find_git_root(cwd).or_else(dirs::home_dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_dir(base: &Path, rel: &str) -> PathBuf {
        let p = base.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // --- user_skills_dir ---

    #[test]
    fn test_user_skills_dir_contains_wcore_skills() {
        if let Some(dir) = user_skills_dir() {
            let s = dir.to_string_lossy();
            assert!(
                s.contains("wayland-core"),
                "expected 'wayland-core' in path: {s}"
            );
            assert!(
                s.ends_with("skills"),
                "expected path to end with 'skills': {s}"
            );
        }
        // If app_config_dir() returns None (rare), that's acceptable.
    }

    #[test]
    fn test_user_commands_dir_contains_wcore_commands() {
        if let Some(dir) = user_commands_dir() {
            let s = dir.to_string_lossy();
            assert!(s.contains("wayland-core"));
            assert!(s.ends_with("commands"));
        }
    }

    // --- find_git_root ---

    #[test]
    fn test_find_git_root_finds_git_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let nested = root.join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(root.join(".git")).unwrap();

        let found = find_git_root(&nested).unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn test_find_git_root_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        // No .git anywhere under tmp
        let result = find_git_root(tmp.path());
        // May or may not find a .git in an ancestor of tmp — we just ensure no panic.
        // If the test environment has a .git above tmp, that's ok.
        let _ = result;
    }

    #[test]
    fn test_find_git_root_at_root_itself() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        let found = find_git_root(tmp.path()).unwrap();
        assert_eq!(found, tmp.path());
    }

    // --- project_skills_dirs ---

    #[test]
    fn test_project_skills_dirs_finds_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Create git root marker
        fs::create_dir(root.join(".git")).unwrap();

        // Create skills dirs at root and nested level
        make_dir(root, ".wayland-core/skills");
        let nested = root.join("sub").join("project");
        fs::create_dir_all(&nested).unwrap();
        make_dir(&nested, ".wayland-core/skills");

        let dirs = project_skills_dirs(&nested);
        // Should find both (deepest first)
        assert_eq!(dirs.len(), 2);
        // First one is deeper (closest to cwd)
        assert!(dirs[0].starts_with(&nested));
        assert!(dirs[1].starts_with(root));
    }

    #[test]
    fn test_project_skills_dirs_skips_missing() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        // No .wayland-core/skills/ anywhere
        let dirs = project_skills_dirs(tmp.path());
        assert!(dirs.is_empty());
    }

    // --- additional_skills_dirs ---

    #[test]
    fn test_additional_skills_dirs_existing() {
        let tmp = TempDir::new().unwrap();
        make_dir(tmp.path(), ".wayland-core/skills");
        let result = additional_skills_dirs(&[tmp.path().to_path_buf()]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_additional_skills_dirs_missing_silently_skipped() {
        let tmp = TempDir::new().unwrap();
        // No .wayland-core/skills/ under tmp
        let result = additional_skills_dirs(&[tmp.path().to_path_buf()]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_additional_skills_dirs_empty_input() {
        let result = additional_skills_dirs(&[]);
        assert!(result.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Supplemental tests (tester role — covers test-plan.md cases not in impl tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod supplemental_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_dir(base: &Path, rel: &str) -> PathBuf {
        let p = base.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // -----------------------------------------------------------------------
    // TC-1.x: find_git_root
    // -----------------------------------------------------------------------

    #[test]
    fn tc_1_1_find_git_root_at_root_dir() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        let found = find_git_root(tmp.path()).unwrap();
        assert_eq!(found, tmp.path());
    }

    #[test]
    fn tc_1_2_find_git_root_from_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        let sub = root.join("src").join("module");
        fs::create_dir_all(&sub).unwrap();

        let found = find_git_root(&sub).unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn tc_1_4_find_git_root_deep_nesting() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        let deep = root.join("a").join("b").join("c").join("d").join("e");
        fs::create_dir_all(&deep).unwrap();

        let found = find_git_root(&deep).unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn tc_1_5_find_git_root_git_is_file_not_dir() {
        // git worktree: .git is a file, not a directory
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join(".git"), "gitdir: ../main/.git/worktrees/wt").unwrap();

        // Implementation uses .exists() which is true for both files and dirs
        let found = find_git_root(root);
        assert!(
            found.is_some(),
            ".git file should be recognized as git root"
        );
        assert_eq!(found.unwrap(), root);
    }

    // -----------------------------------------------------------------------
    // TC-2.x / TC-3.x: user_skills_dir / user_commands_dir
    // -----------------------------------------------------------------------

    #[test]
    fn tc_2_1_user_skills_dir_ends_with_skills() {
        if let Some(dir) = user_skills_dir() {
            let s = dir.to_string_lossy();
            assert!(s.ends_with("skills"), "path should end with 'skills': {s}");
            assert!(
                s.contains("wayland-core"),
                "path should contain 'wayland-core': {s}"
            );
        }
    }

    #[test]
    fn tc_3_1_user_commands_dir_ends_with_commands() {
        if let Some(dir) = user_commands_dir() {
            let s = dir.to_string_lossy();
            assert!(
                s.ends_with("commands"),
                "path should end with 'commands': {s}"
            );
            assert!(
                s.contains("wayland-core"),
                "path should contain 'wayland-core': {s}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // TC-4.x: project_skills_dirs
    // -----------------------------------------------------------------------

    #[test]
    fn tc_4_2_project_skills_dirs_nonexistent_subdir_not_returned() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        // No .wayland-core/skills/ created
        let dirs = project_skills_dirs(tmp.path());
        assert!(
            dirs.is_empty(),
            "should be empty when .wayland-core/skills/ doesn't exist"
        );
    }

    #[test]
    fn tc_4_3_project_skills_dirs_deepest_first() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        make_dir(root, ".wayland-core/skills");

        let inner = root.join("sub");
        fs::create_dir_all(&inner).unwrap();
        make_dir(&inner, ".wayland-core/skills");

        let dirs = project_skills_dirs(&inner);
        assert_eq!(dirs.len(), 2);
        // First element should be closest to cwd (deepest)
        assert!(
            dirs[0].starts_with(&inner),
            "first dir should be the inner one (deepest): {:?}",
            dirs[0]
        );
    }

    #[test]
    fn tc_4_4_project_skills_dirs_stops_at_git_root() {
        let tmp = TempDir::new().unwrap();
        let grandparent = tmp.path();
        // .wayland-core/skills in grandparent (above git root) — should NOT be collected
        make_dir(grandparent, ".wayland-core/skills");

        let repo = grandparent.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir(repo.join(".git")).unwrap();
        make_dir(&repo, ".wayland-core/skills");

        let sub = repo.join("sub");
        fs::create_dir_all(&sub).unwrap();

        let dirs = project_skills_dirs(&sub);
        // Only repo's .wayland-core/skills should be included
        assert!(
            dirs.iter().all(|d| d.starts_with(&repo)),
            "should not include dirs above git root, got: {dirs:?}"
        );
        assert_eq!(dirs.len(), 1);
    }

    #[test]
    fn tc_4_6_project_skills_dirs_nonexistent_cwd_no_panic() {
        // Should not panic even if cwd does not exist
        let dirs = project_skills_dirs(Path::new("/tmp/nonexistent_cwd_xyz_abc_123"));
        // Result may be empty or not (depends on ancestor dirs) — just must not panic
        let _ = dirs;
    }

    // -----------------------------------------------------------------------
    // TC-5.x: project_commands_dirs
    // -----------------------------------------------------------------------

    #[test]
    fn tc_5_1_project_commands_dirs_finds_commands_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        make_dir(root, ".wayland-core/commands");

        let dirs = project_commands_dirs(root);
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with(".wayland-core/commands"));
    }

    // -----------------------------------------------------------------------
    // TC-6.x: additional_skills_dirs
    // -----------------------------------------------------------------------

    #[test]
    fn tc_6_1_additional_skills_dirs_with_existing_subdir() {
        let tmp = TempDir::new().unwrap();
        make_dir(tmp.path(), ".wayland-core/skills");

        let result = additional_skills_dirs(&[tmp.path().to_path_buf()]);
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with(".wayland-core/skills"));
    }

    #[test]
    fn tc_6_2_additional_skills_dirs_no_subdir_skipped() {
        let tmp = TempDir::new().unwrap();
        // No .wayland-core/skills/ subdirectory
        let result = additional_skills_dirs(&[tmp.path().to_path_buf()]);
        assert!(result.is_empty());
    }

    #[test]
    fn tc_6_4_additional_skills_dirs_multiple_add_dirs() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        make_dir(tmp1.path(), ".wayland-core/skills");
        make_dir(tmp2.path(), ".wayland-core/skills");

        let result =
            additional_skills_dirs(&[tmp1.path().to_path_buf(), tmp2.path().to_path_buf()]);
        assert_eq!(result.len(), 2);
    }
}
