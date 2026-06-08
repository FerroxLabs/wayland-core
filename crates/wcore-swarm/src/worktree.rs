//! WorktreeManager — git worktree create/cleanup for swarm workers.
//!
//! All git subprocess calls flow through
//! [`wcore_config::shell::shell_command_argv`] (argv mode — no shell
//! interpretation), per AGENTS.md cross-platform rules. Working directory
//! is set with `.current_dir(...)` on the returned `tokio::process::Command`.

use std::path::{Path, PathBuf};

use wcore_config::shell;

use crate::error::{Result, SwarmError};

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
        let swarm_root = repo_root.join(".swarm-worktrees");
        std::fs::create_dir_all(&swarm_root)?;
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
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
        let tree_path = self.swarm_root.join(worker_id);
        let tree_path_str = tree_path.to_string_lossy().into_owned();
        let args: [&str; 6] = [
            "worktree",
            "add",
            "-b",
            branch,
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
