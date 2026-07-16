//! WorktreeManager — git worktree create/cleanup for swarm workers.
//!
//! All git subprocess calls flow through
//! [`wcore_config::shell::shell_command_argv`] (argv mode — no shell
//! interpretation), per AGENTS.md cross-platform rules. Working directory
//! is set with `.current_dir(...)` on the returned `tokio::process::Command`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use wcore_config::shell;
use wcore_sandbox::process_capture::{CaptureLimits, ProcessCaptureError, capture_bounded_process};

use crate::error::{Result, SwarmError};

#[path = "worktree_security.rs"]
mod security;
use security::{
    ensure_absent_destination, ensure_real_directory, ensure_unchanged_real_directory,
    is_real_directory_entry, make_guard_dir_private, reject_option_like_ref, validate_worker_id,
    write_empty_private_config,
};

/// Manages the `<repo>/.swarm-worktrees/` directory and per-worker
/// worktrees within it. Each worker gets a fresh checkout at
/// `<repo>/.swarm-worktrees/<worker_id>` on a branch named by
/// [`super::SwarmBrief::worker_branch_prefix`] + `/` + `worker_id`.
pub struct WorktreeManager {
    repo_root: PathBuf,
    swarm_root: PathBuf,
    git_program: String,
    capture_limits: CaptureLimits,
    _git_guard_dir: tempfile::TempDir,
    empty_git_config: PathBuf,
    disabled_hooks: PathBuf,
    #[cfg(test)]
    ambient_git_env: Vec<(String, std::ffi::OsString)>,
}

const CLEANUP_GRACE: Duration = Duration::from_secs(5);
const GIT_CAPTURE_LIMITS: CaptureLimits = CaptureLimits {
    stdout_bytes: 1024 * 1024,
    stderr_bytes: 256 * 1024,
    timeout: Duration::from_secs(120),
};
const UNSAFE_CHECKOUT_CONFIG: &str =
    r"^(filter\..*\.(clean|smudge|process)|include\.path|includeif\..*\.path)$";

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
        let git_guard_dir = tempfile::Builder::new()
            .prefix("wayland-swarm-git-")
            .tempdir()?;
        make_guard_dir_private(git_guard_dir.path())?;
        let empty_git_config = git_guard_dir.path().join("empty-gitconfig");
        write_empty_private_config(&empty_git_config)?;
        let disabled_hooks = git_guard_dir.path().join("disabled-hooks");
        Ok(Self {
            repo_root,
            swarm_root,
            git_program: "git".to_string(),
            capture_limits: GIT_CAPTURE_LIMITS,
            _git_guard_dir: git_guard_dir,
            empty_git_config,
            disabled_hooks,
            #[cfg(test)]
            ambient_git_env: Vec::new(),
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
        self.reject_executable_checkout_config().await?;
        let cmd = self.git_command(&["status", "--porcelain"]);
        let out = capture_bounded_process(cmd, self.capture_limits, None)
            .await
            .map_err(|error| capture_error("git status", error))?;
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
        let tree_path = self.swarm_root.join(worker_id);
        self.reject_executable_checkout_config().await?;
        self.validate_swarm_root()?;
        // Git has no descriptor-relative worktree-add API, so there is an
        // unavoidable same-UID race after this check. Keep the window to the
        // final pre-spawn step and fail closed on an existing destination.
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
        let cmd = self.git_command(&args);
        let out = match capture_bounded_process(cmd, self.capture_limits, None).await {
            Ok(output) => output,
            Err(error) => {
                return Err(worktree_add_error(
                    &tree_path,
                    format!("git worktree add: {error}"),
                ));
            }
        };
        if !out.status.success() {
            return Err(worktree_add_error(
                &tree_path,
                format!(
                    "git worktree add failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(tree_path)
    }

    /// Remove every directory under `.swarm-worktrees/` via
    /// `git worktree remove --force`. Attempts every safe entry, then reports
    /// all failures and every residual path. Idempotent when the root is empty.
    pub async fn cleanup_all(&self, escalation: &CancellationToken) -> Result<()> {
        let deadline = tokio::time::Instant::now() + CLEANUP_GRACE;
        let mut failures = Vec::new();
        let mut entries = Vec::new();
        match std::fs::symlink_metadata(&self.swarm_root) {
            Ok(_) => {
                self.validate_swarm_root()?;
                for entry in std::fs::read_dir(&self.swarm_root)? {
                    match entry {
                        Ok(entry) => {
                            let path = entry.path();
                            match is_real_directory_entry(&path) {
                                Ok(true) => entries.push(path),
                                Ok(false) => failures.push(format!(
                                    "refused non-directory cleanup entry: {}",
                                    path.display()
                                )),
                                Err(error) => failures.push(error.to_string()),
                            }
                        }
                        Err(error) => failures.push(format!(
                            "failed to enumerate worktree cleanup entry: {error}"
                        )),
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }

        let mut interrupted = None;
        for path in entries {
            if let Err(error) = self.validate_swarm_root() {
                failures.push(error.to_string());
                break;
            }
            match is_real_directory_entry(&path) {
                Ok(true) => {}
                Ok(false) => {
                    failures.push(format!(
                        "worktree cleanup entry changed before removal: {}",
                        path.display()
                    ));
                    continue;
                }
                Err(error) => {
                    failures.push(error.to_string());
                    continue;
                }
            }
            let path_str = path.to_string_lossy().into_owned();
            let args: [&str; 4] = ["worktree", "remove", "--force", path_str.as_str()];
            let cmd = self.git_command(&args);
            match self.capture_cleanup(cmd, escalation, deadline).await {
                Ok(output) if output.status.success() => {}
                Ok(output) => failures.push(format!(
                    "{}: {}",
                    path.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(ProcessCaptureError::Cancelled) => {
                    interrupted = Some("cleanup escalated by cancellation");
                    break;
                }
                Err(ProcessCaptureError::Timeout(_)) => {
                    interrupted = Some("cleanup exceeded its five-second deadline");
                    break;
                }
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        if interrupted.is_none() {
            let prune = self.git_command(&["worktree", "prune"]);
            match self.capture_cleanup(prune, escalation, deadline).await {
                Ok(output) if output.status.success() => {}
                Ok(output) => failures.push(format!(
                    "git worktree prune: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(ProcessCaptureError::Cancelled) => {
                    interrupted = Some("cleanup escalated by cancellation")
                }
                Err(ProcessCaptureError::Timeout(_)) => {
                    interrupted = Some("cleanup exceeded its five-second deadline")
                }
                Err(error) => failures.push(format!("git worktree prune: {error}")),
            }
        }
        if let Some(reason) = interrupted {
            failures.push(format!("{reason}; Git worktree metadata may remain"));
        }

        for entry in std::fs::read_dir(&self.swarm_root)? {
            match entry {
                Ok(entry) => failures.push(format!(
                    "residual worktree path: {}",
                    entry.path().display()
                )),
                Err(error) => failures.push(format!(
                    "failed to enumerate residual worktree entry: {error}"
                )),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(SwarmError::Cleanup(failures.join("; ")))
        }
    }

    #[cfg(test)]
    fn new_with_git_program(repo_root: &Path, git_program: &Path) -> Result<Self> {
        let mut manager = Self::new(repo_root)?;
        manager.git_program = git_program.to_string_lossy().into_owned();
        Ok(manager)
    }

    #[cfg(test)]
    fn new_with_git_program_and_limits(
        repo_root: &Path,
        git_program: &Path,
        capture_limits: CaptureLimits,
    ) -> Result<Self> {
        let mut manager = Self::new_with_git_program(repo_root, git_program)?;
        manager.capture_limits = capture_limits;
        Ok(manager)
    }

    #[cfg(test)]
    fn set_ambient_git_env(&mut self, key: &str, value: impl Into<std::ffi::OsString>) {
        self.ambient_git_env.push((key.to_string(), value.into()));
    }

    async fn reject_executable_checkout_config(&self) -> Result<()> {
        // System/global configuration is disabled for every Swarm Git command.
        // Inspect both repository-local scopes without following includes so
        // executable filters and conditional/external includes fail closed
        // before checkout. `--local` does not include `config.worktree` when
        // `extensions.worktreeConfig` is enabled.
        for scope in ["--local", "--worktree"] {
            let cmd = self.git_command(&[
                "config",
                scope,
                "--no-includes",
                "--name-only",
                "--get-regexp",
                UNSAFE_CHECKOUT_CONFIG,
            ]);
            let output = capture_bounded_process(cmd, self.capture_limits, None)
                .await
                .map_err(|error| capture_error("git config safety check", error))?;
            if output.status.success() {
                let keys = String::from_utf8_lossy(&output.stdout);
                return Err(SwarmError::WorktreeIo(format!(
                    "refused executable or conditional Git checkout configuration in {scope}: {}",
                    keys.trim()
                )));
            }
            if output.status.code() == Some(1) && output.stdout.is_empty() {
                continue;
            }
            return Err(SwarmError::WorktreeIo(format!(
                "git config safety check failed for {scope}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    fn git_command(&self, args: &[&str]) -> tokio::process::Command {
        let hooks = format!("core.hooksPath={}", self.disabled_hooks.display());
        let mut protected_args = vec![
            "-c".to_string(),
            hooks,
            "-c".to_string(),
            "core.fsmonitor=false".to_string(),
            "-c".to_string(),
            "core.autocrlf=false".to_string(),
        ];
        protected_args.extend(args.iter().map(|arg| (*arg).to_string()));
        let argv = protected_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let mut cmd = shell::shell_command_argv(&self.git_program, &argv);
        #[cfg(test)]
        for (key, value) in &self.ambient_git_env {
            cmd.env(key, value);
        }
        cmd.current_dir(&self.repo_root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", &self.empty_git_config)
            .env("GIT_CONFIG_GLOBAL", &self.empty_git_config)
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "Never")
            .env_remove("GIT_CONFIG")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_DIR")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .env_remove("GIT_EXEC_PATH")
            .env_remove("GIT_EXTERNAL_DIFF")
            .env_remove("GIT_DIFF_OPTS")
            .env_remove("GIT_PAGER")
            .env_remove("GIT_EDITOR")
            .env_remove("GIT_SEQUENCE_EDITOR")
            .env_remove("GIT_SSH")
            .env_remove("GIT_SSH_COMMAND")
            .env_remove("GIT_ASKPASS")
            .env_remove("SSH_ASKPASS");
        cmd
    }

    fn validate_swarm_root(&self) -> Result<()> {
        ensure_unchanged_real_directory(&self.swarm_root, &self.repo_root)
    }

    async fn capture_cleanup(
        &self,
        command: tokio::process::Command,
        escalation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> std::result::Result<wcore_sandbox::process_capture::CapturedOutput, ProcessCaptureError>
    {
        let mut limits = self.capture_limits;
        limits.timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
        capture_bounded_process(command, limits, Some(escalation)).await
    }
}

fn capture_error(context: &str, error: ProcessCaptureError) -> SwarmError {
    SwarmError::WorktreeIo(format!("{context}: {error}"))
}

fn worktree_add_error(path: &Path, reason: String) -> SwarmError {
    let residual = match std::fs::symlink_metadata(path) {
        Ok(_) => format!("; residual worktree path preserved: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => format!(
            "; could not inspect possible residual worktree path {}: {error}",
            path.display()
        ),
    };
    SwarmError::WorktreeIo(format!("{reason}{residual}"))
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
