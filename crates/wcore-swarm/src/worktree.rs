//! WorktreeManager — git worktree create/cleanup for swarm workers.
//!
//! All git subprocess calls flow through
//! [`wcore_config::shell::shell_command_argv`] (argv mode — no shell
//! interpretation), per AGENTS.md cross-platform rules. Working directory
//! is set with `.current_dir(...)` on the returned `tokio::process::Command`.

use std::path::{Path, PathBuf};

use wcore_config::shell;

use crate::error::{Result, SwarmError};

#[path = "worktree_security.rs"]
mod security;
use security::{
    ensure_absent_destination, ensure_real_directory, ensure_unchanged_real_directory,
    reject_option_like_ref, validate_worker_id,
};

/// Manages the `<repo>/.swarm-worktrees/` directory and per-worker
/// worktrees within it. Each worker gets a fresh checkout at
/// `<repo>/.swarm-worktrees/<worker_id>` on a branch named by
/// [`super::SwarmBrief::worker_branch_prefix`] + `/` + `worker_id`.
pub struct WorktreeManager {
    repo_root: PathBuf,
    swarm_root: PathBuf,
}

impl WorktreeManager {
    /// Construct a new manager for `repo_root`. Creates the
    /// `.swarm-worktrees/` directory if it does not exist.
    pub fn new(repo_root: &Path) -> Result<Self> {
        let repo_root = std::fs::canonicalize(repo_root)?;
        let swarm_root = repo_root.join(".swarm-worktrees");
        ensure_real_directory(&swarm_root)?;
        let swarm_root = std::fs::canonicalize(&swarm_root)?;
        if swarm_root.parent() != Some(repo_root.as_path()) {
            return Err(SwarmError::WorktreeIo(format!(
                "refused worktree root outside repository: {}",
                swarm_root.display()
            )));
        }
        Ok(Self {
            repo_root,
            swarm_root,
        })
    }

    /// Return the underlying repository root.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Return the swarm worktree root (`<repo>/.swarm-worktrees/`).
    pub fn swarm_root(&self) -> &Path {
        &self.swarm_root
    }

    /// Reject dispatch on a dirty checkout. Runs `git status --porcelain`
    /// in `repo_root` and returns [`SwarmError::DirtyCheckout`] if the
    /// output is non-empty.
    ///
    /// This is the collision-detection gate that prevents the v0.2.2
    /// incident (dirty worker contaminating main).
    pub async fn assert_clean(&self) -> Result<()> {
        let mut cmd = shell::shell_command_argv("git", &["status", "--porcelain"]);
        cmd.current_dir(&self.repo_root);
        let out = cmd
            .output()
            .await
            .map_err(|e| SwarmError::WorktreeIo(format!("git status: {e}")))?;
        if !out.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "git status failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !stdout.trim().is_empty() {
            return Err(SwarmError::DirtyCheckout(stdout.trim().to_string()));
        }
        Ok(())
    }

    /// Create a fresh worktree at `<swarm_root>/<worker_id>` on a new
    /// branch `branch` checked out from `base`. Returns the worktree path.
    pub async fn create_worker_tree(
        &self,
        worker_id: &str,
        branch: &str,
        base: &str,
    ) -> Result<PathBuf> {
        validate_worker_id(worker_id)?;
        reject_option_like_ref("branch", branch)?;
        reject_option_like_ref("base", base)?;
        ensure_unchanged_real_directory(&self.swarm_root, &self.repo_root)?;
        let tree_path = self.swarm_root.join(worker_id);
        ensure_absent_destination(&tree_path)?;
        let tree_path_str = tree_path.to_string_lossy().into_owned();
        let args: [&str; 7] = [
            "worktree",
            "add",
            "-b",
            branch,
            "--",
            tree_path_str.as_str(),
            base,
        ];
        let mut cmd = shell::shell_command_argv("git", &args);
        cmd.current_dir(&self.repo_root);
        let out = cmd
            .output()
            .await
            .map_err(|e| SwarmError::WorktreeIo(format!("worktree add: {e}")))?;
        if !out.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(tree_path)
    }

    /// Remove every directory under `.swarm-worktrees/` via
    /// `git worktree remove --force`. Best-effort and idempotent: a
    /// failure on one entry is logged but does not abort the loop.
    pub async fn cleanup_all(&self) -> Result<()> {
        if !self.swarm_root.exists() {
            return Ok(());
        }
        let entries: Vec<PathBuf> = std::fs::read_dir(&self.swarm_root)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        for path in entries {
            let path_str = path.to_string_lossy().into_owned();
            let args: [&str; 4] = ["worktree", "remove", "--force", path_str.as_str()];
            let mut cmd = shell::shell_command_argv("git", &args);
            cmd.current_dir(&self.repo_root);
            if let Err(e) = cmd.status().await {
                tracing::warn!(?path, error = %e, "worktree cleanup failed; continuing");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_worker_and_ref_inputs_fail_before_git_dispatch() {
        let repo = tempfile::tempdir().unwrap();
        let manager = WorktreeManager::new(repo.path()).unwrap();

        for worker_id in ["../escape", "nested/worker", "", "."] {
            let error = manager
                .create_worker_tree(worker_id, "worker/safe", "main")
                .await
                .unwrap_err();
            assert!(error.to_string().contains("invalid worker id"));
        }
        for (branch, base) in [("--orphan", "main"), ("worker/safe", "-C")] {
            let error = manager
                .create_worker_tree("safe-worker", branch, base)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("invalid"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_swarm_root_is_rejected_without_touching_target() {
        use std::os::unix::fs::symlink;

        let repo = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        symlink(target.path(), repo.path().join(".swarm-worktrees")).unwrap();

        let error = match WorktreeManager::new(repo.path()) {
            Ok(_) => panic!("linked swarm root was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("linked worktree root"));
        assert!(std::fs::read_dir(target.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn linked_worker_destination_is_rejected_before_git_dispatch() {
        use std::os::unix::fs::symlink;

        let repo = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let manager = WorktreeManager::new(repo.path()).unwrap();
        symlink(target.path(), manager.swarm_root().join("safe-worker")).unwrap();

        let error = manager
            .create_worker_tree("safe-worker", "worker/safe", "main")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("existing or linked"));
        assert!(std::fs::read_dir(target.path()).unwrap().next().is_none());
    }
}
