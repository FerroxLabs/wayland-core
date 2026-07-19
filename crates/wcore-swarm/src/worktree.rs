//! WorktreeManager — git worktree create/cleanup for swarm workers.
//!
//! All git subprocess calls flow through
//! [`wcore_config::shell::shell_command_argv`] (argv mode — no shell
//! interpretation), per AGENTS.md cross-platform rules. Working directory
//! is set with `.current_dir(...)` on the returned `tokio::process::Command`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::Mutex;
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
    swarm_parent: PathBuf,
    git_program: String,
    capture_limits: CaptureLimits,
    _git_guard_dir: tempfile::TempDir,
    empty_git_config: PathBuf,
    disabled_hooks: PathBuf,
    admission_lock: Mutex<()>,
    #[cfg(test)]
    ambient_git_env: Vec<(String, std::ffi::OsString)>,
}

/// Parent-issued storage proof for one delegated mutation checkout.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceCapacity {
    pub available_bytes: u64,
    pub safety_margin_bytes: u64,
    pub max_transaction_bytes: u64,
    pub max_aggregate_bytes: u64,
}

/// Identity-bound roots owned by one delegated mutation transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionWorkspace {
    pub owner: String,
    pub root: PathBuf,
    pub checkout: PathBuf,
    pub scratch: PathBuf,
    pub base_commit: String,
    pub head_commit: String,
    pub tree: String,
    pub reserved_bytes: u64,
}

const RESERVATION_FILE: &str = ".wayland-reservation";

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
            swarm_parent: repo_root.clone(),
            repo_root,
            swarm_root,
            git_program: "git".to_string(),
            capture_limits: GIT_CAPTURE_LIMITS,
            _git_guard_dir: git_guard_dir,
            empty_git_config,
            disabled_hooks,
            admission_lock: Mutex::new(()),
            #[cfg(test)]
            ambient_git_env: Vec::new(),
        })
    }

    /// Construct a manager whose child-controlled checkouts live in an
    /// orchestrator-owned directory outside the source repository.
    ///
    /// The Git common directory remains owned by the parent orchestrator. A
    /// child receives only its checkout path; callers must keep the repository
    /// root and [`Self::git_common_dir`] outside the child's sandbox grants.
    pub fn new_with_workspace_root(repo_root: &Path, workspace_root: &Path) -> Result<Self> {
        if !workspace_root.is_absolute() {
            return Err(SwarmError::WorktreeIo(
                "orchestrator worktree root must be absolute".to_owned(),
            ));
        }
        let repo_root = std::fs::canonicalize(repo_root)?;
        let workspace_parent = workspace_root.parent().ok_or_else(|| {
            SwarmError::WorktreeIo("orchestrator worktree root has no parent".to_owned())
        })?;
        std::fs::create_dir_all(workspace_parent)?;
        let workspace_parent = std::fs::canonicalize(workspace_parent)?;
        if workspace_parent.starts_with(&repo_root) || repo_root.starts_with(&workspace_parent) {
            return Err(SwarmError::WorktreeIo(format!(
                "orchestrator worktree root must not overlap repository {}",
                repo_root.display()
            )));
        }
        ensure_real_directory(workspace_root)?;
        make_guard_dir_private(workspace_root)?;
        let swarm_root = std::fs::canonicalize(workspace_root)?;
        if swarm_root.parent() != Some(workspace_parent.as_path()) {
            return Err(SwarmError::WorktreeIo(format!(
                "refused worktree root outside orchestrator directory: {}",
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
            swarm_parent: workspace_parent,
            git_program: "git".to_string(),
            capture_limits: GIT_CAPTURE_LIMITS,
            _git_guard_dir: git_guard_dir,
            empty_git_config,
            disabled_hooks,
            admission_lock: Mutex::new(()),
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

    /// Count retained worker worktrees without following linked entries.
    /// Enumeration stops once `stop_after` is exceeded so an already-invalid
    /// worktree root cannot force an unbounded admission scan.
    pub fn retained_worker_count(&self, stop_after: usize) -> Result<usize> {
        self.validate_swarm_root()?;
        let mut count = 0_usize;
        for entry in std::fs::read_dir(&self.swarm_root)? {
            let path = entry?.path();
            if !is_real_directory_entry(&path)? {
                return Err(SwarmError::WorktreeIo(format!(
                    "refused non-directory worktree entry during admission: {}",
                    path.display()
                )));
            }
            count = count.checked_add(1).ok_or_else(|| {
                SwarmError::DispatchAdmission("retained worktree count overflowed".into())
            })?;
            if count > stop_after {
                break;
            }
        }
        Ok(count)
    }

    /// Resolve the parent repository's common Git administration directory
    /// under the same scrubbed Git environment used for worktree operations.
    pub async fn git_common_dir(&self) -> Result<PathBuf> {
        self.reject_executable_checkout_config().await?;
        let cmd = self.git_command(&["rev-parse", "--git-common-dir"]);
        let out = capture_bounded_process(cmd, self.capture_limits, None)
            .await
            .map_err(|error| capture_error("git rev-parse common dir", error))?;
        if !out.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "git rev-parse common dir failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        let path = if path.is_absolute() {
            path
        } else {
            self.repo_root.join(path)
        };
        std::fs::canonicalize(&path).map_err(Into::into)
    }

    /// Resolve the exact commit currently named by `HEAD` without consulting
    /// ambient Git configuration. Delegated worktrees must branch from this
    /// immutable object id rather than re-resolving a moving symbolic ref
    /// after admission.
    pub async fn pinned_head(&self) -> Result<String> {
        self.reject_executable_checkout_config().await?;
        self.read_pinned_head().await
    }

    async fn read_pinned_head(&self) -> Result<String> {
        let cmd = self.git_command(&["rev-parse", "--verify", "HEAD^{commit}"]);
        let out = capture_bounded_process(cmd, self.capture_limits, None)
            .await
            .map_err(|error| capture_error("git rev-parse HEAD", error))?;
        if !out.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "git rev-parse HEAD failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let commit = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(SwarmError::WorktreeIo(
                "git rev-parse HEAD returned an invalid object id".to_owned(),
            ));
        }
        Ok(commit)
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

    /// Create a private, single-commit Git checkout for a delegated agent.
    ///
    /// Unlike `git worktree add`, this checkout has its own object store and
    /// administration directory. The child can therefore use ordinary local
    /// Git inspection without receiving access to the parent's refs, object
    /// store, hooks, remotes, tags, or history.
    pub async fn create_isolated_checkout(
        &self,
        worker_id: &str,
        branch: &str,
        pinned_head: &str,
        capacity: WorkspaceCapacity,
    ) -> Result<TransactionWorkspace> {
        Box::pin(self.create_isolated_checkout_inner(worker_id, branch, pinned_head, capacity))
            .await
    }

    // Keep the construction future behind one allocation. This operation
    // carries several bounded process-capture states; inlining all of them in
    // a caller's async state machine can exhaust the default test-thread stack.
    async fn create_isolated_checkout_inner(
        &self,
        worker_id: &str,
        branch: &str,
        pinned_head: &str,
        capacity: WorkspaceCapacity,
    ) -> Result<TransactionWorkspace> {
        validate_worker_id(worker_id)?;
        reject_option_like_ref("branch", branch)?;
        reject_option_like_ref("base", pinned_head)?;
        if !matches!(pinned_head.len(), 40 | 64)
            || !pinned_head.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(SwarmError::WorktreeIo(
                "isolated checkout requires an exact commit id".to_owned(),
            ));
        }

        self.reject_executable_checkout_config().await?;
        self.validate_swarm_root()?;
        let _admission = self.admission_lock.lock().await;
        let closure_bytes = self.transfer_closure_bytes(pinned_head).await?;
        let aggregate = self.reserved_workspace_bytes()?;
        let required = closure_bytes
            .checked_add(capacity.safety_margin_bytes)
            .ok_or_else(|| SwarmError::WorktreeIo("workspace capacity overflow".to_owned()))?;
        if closure_bytes > capacity.max_transaction_bytes {
            return Err(SwarmError::DispatchAdmission(format!(
                "workspace closure {closure_bytes} exceeds transaction budget {}",
                capacity.max_transaction_bytes
            )));
        }
        if aggregate
            .checked_add(closure_bytes)
            .is_none_or(|total| total > capacity.max_aggregate_bytes)
        {
            return Err(SwarmError::DispatchAdmission(
                "aggregate workspace budget exhausted".to_owned(),
            ));
        }
        if required > capacity.available_bytes {
            return Err(SwarmError::DispatchAdmission(format!(
                "workspace requires {required} available bytes but authority proved only {}",
                capacity.available_bytes
            )));
        }

        let transaction_root = self.swarm_root.join(worker_id);
        ensure_absent_destination(&transaction_root)?;
        std::fs::create_dir(&transaction_root)?;
        make_guard_dir_private(&transaction_root)?;
        let reservation = transaction_root.join(RESERVATION_FILE);
        std::fs::write(&reservation, closure_bytes.to_string())?;
        let checkout = transaction_root.join("checkout");
        let scratch = transaction_root.join("scratch");
        std::fs::create_dir(&scratch)?;
        make_guard_dir_private(&scratch)?;
        let source = self.repo_root.to_string_lossy().into_owned();
        let destination = checkout.to_string_lossy().into_owned();
        let clone_args = [
            "clone",
            "--no-local",
            "--no-hardlinks",
            "--depth=1",
            "--no-tags",
            "--single-branch",
            "--no-checkout",
            "--",
            source.as_str(),
            destination.as_str(),
        ];
        let clone = Box::pin(capture_bounded_process(
            self.git_command(&clone_args),
            self.capture_limits,
            None,
        ))
        .await;
        let clone = match clone {
            Ok(output) => output,
            Err(error) => {
                self.remove_owned_transaction_root(worker_id, &transaction_root)?;
                return Err(SwarmError::WorktreeIo(format!(
                    "isolated git clone failed: {error}"
                )));
            }
        };
        if !clone.status.success() {
            self.remove_owned_transaction_root(worker_id, &transaction_root)?;
            return Err(SwarmError::WorktreeIo(format!(
                "isolated git clone failed: {}",
                String::from_utf8_lossy(&clone.stderr).trim()
            )));
        }
        make_guard_dir_private(&checkout)?;

        Box::pin(self.run_checkout_git(&checkout, &["remote", "remove", "origin"])).await?;
        let actual = Box::pin(
            self.checkout_git_stdout(&checkout, &["rev-parse", "--verify", "HEAD^{commit}"]),
        )
        .await?;
        if actual != pinned_head {
            return Err(SwarmError::WorktreeIo(format!(
                "isolated checkout raced parent HEAD: expected {pinned_head}, got {actual}"
            )));
        }
        Box::pin(self.run_checkout_git(&checkout, &["checkout", "-b", branch, pinned_head, "--"]))
            .await?;

        let common =
            Box::pin(self.checkout_git_stdout(&checkout, &["rev-parse", "--git-common-dir"]))
                .await?;
        let common = PathBuf::from(common);
        let common = if common.is_absolute() {
            common
        } else {
            checkout.join(common)
        };
        let common = std::fs::canonicalize(common)?;
        if !common.starts_with(&checkout) {
            return Err(SwarmError::WorktreeIo(format!(
                "isolated checkout Git authority escaped its root: {}",
                common.display()
            )));
        }
        let alternates = common.join("objects").join("info").join("alternates");
        if std::fs::symlink_metadata(&alternates).is_ok() {
            return Err(SwarmError::WorktreeIo(
                "isolated checkout unexpectedly uses an alternate object store".to_owned(),
            ));
        }
        let remotes = Box::pin(self.checkout_git_stdout(&checkout, &["remote"])).await?;
        if !remotes.is_empty() {
            return Err(SwarmError::WorktreeIo(
                "isolated checkout retained a remote".to_owned(),
            ));
        }
        let tags = Box::pin(self.checkout_git_stdout(&checkout, &["tag", "--list"])).await?;
        if !tags.is_empty() {
            return Err(SwarmError::WorktreeIo(
                "isolated checkout retained tags".to_owned(),
            ));
        }
        let reachable =
            Box::pin(self.checkout_git_stdout(&checkout, &["rev-list", "--count", "--all"]))
                .await?;
        if reachable != "1" {
            return Err(SwarmError::WorktreeIo(format!(
                "isolated checkout retained {reachable} reachable commits"
            )));
        }
        if Box::pin(self.read_pinned_head()).await? != pinned_head {
            return Err(SwarmError::WorktreeIo(
                "parent HEAD changed during isolated checkout preparation".to_owned(),
            ));
        }
        let head_commit = Box::pin(
            self.checkout_git_stdout(&checkout, &["rev-parse", "--verify", "HEAD^{commit}"]),
        )
        .await?;
        let tree = Box::pin(
            self.checkout_git_stdout(&checkout, &["rev-parse", "--verify", "HEAD^{tree}"]),
        )
        .await?;
        let checkout = std::fs::canonicalize(checkout)?;
        let scratch = std::fs::canonicalize(scratch)?;
        let transaction_root = std::fs::canonicalize(transaction_root)?;
        if !checkout.starts_with(&transaction_root)
            || !scratch.starts_with(&transaction_root)
            || checkout.starts_with(&scratch)
            || scratch.starts_with(&checkout)
        {
            return Err(SwarmError::WorktreeIo(
                "transaction checkout and scratch roots are not disjoint".to_owned(),
            ));
        }
        Ok(TransactionWorkspace {
            owner: worker_id.to_owned(),
            root: transaction_root,
            checkout,
            scratch,
            base_commit: pinned_head.to_owned(),
            head_commit,
            tree,
            reserved_bytes: closure_bytes,
        })
    }

    async fn transfer_closure_bytes(&self, pinned_head: &str) -> Result<u64> {
        let output = capture_bounded_process(
            self.git_command(&["rev-list", "--disk-usage", "--objects", pinned_head, "--"]),
            self.capture_limits,
            None,
        )
        .await
        .map_err(|error| capture_error("git closure measurement", error))?;
        if !output.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "git closure measurement failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .map_err(|_| SwarmError::WorktreeIo("invalid Git closure measurement".to_owned()))
    }

    fn reserved_workspace_bytes(&self) -> Result<u64> {
        let mut total = 0_u64;
        for entry in std::fs::read_dir(&self.swarm_root)? {
            let entry = entry?;
            if !is_real_directory_entry(&entry.path())? {
                return Err(SwarmError::WorktreeIo(format!(
                    "refused linked workspace reservation entry: {}",
                    entry.path().display()
                )));
            }
            let reservation = entry.path().join(RESERVATION_FILE);
            if reservation.is_file() {
                let bytes = std::fs::read_to_string(&reservation)?
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| {
                        SwarmError::WorktreeIo(format!(
                            "invalid workspace reservation: {}",
                            reservation.display()
                        ))
                    })?;
                total = total.checked_add(bytes).ok_or_else(|| {
                    SwarmError::WorktreeIo("aggregate workspace reservation overflow".to_owned())
                })?;
            }
        }
        Ok(total)
    }

    fn remove_owned_transaction_root(&self, owner: &str, root: &Path) -> Result<()> {
        validate_worker_id(owner)?;
        self.validate_swarm_root()?;
        if root != self.swarm_root.join(owner) || !root.starts_with(&self.swarm_root) {
            return Err(SwarmError::WorktreeIo(
                "refused cleanup outside owned transaction root".to_owned(),
            ));
        }
        match std::fs::remove_dir_all(root) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Release a completed transaction and its persisted capacity reservation.
    pub fn release_transaction(&self, workspace: &TransactionWorkspace) -> Result<()> {
        self.remove_owned_transaction_root(&workspace.owner, &workspace.root)
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
            if path.join(RESERVATION_FILE).is_file() {
                let owner = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        SwarmError::WorktreeIo(format!(
                            "transaction workspace has invalid owner path: {}",
                            path.display()
                        ))
                    })?;
                match self.remove_owned_transaction_root(owner, &path) {
                    Ok(()) => continue,
                    Err(error) => {
                        failures.push(format!("{}: {error}", path.display()));
                        continue;
                    }
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

    fn checkout_git_command(&self, checkout: &Path, args: &[&str]) -> tokio::process::Command {
        let checkout = checkout.to_string_lossy().into_owned();
        let mut scoped = vec!["-C", checkout.as_str()];
        scoped.extend_from_slice(args);
        self.git_command(&scoped)
    }

    async fn run_checkout_git(&self, checkout: &Path, args: &[&str]) -> Result<()> {
        let output = capture_bounded_process(
            self.checkout_git_command(checkout, args),
            self.capture_limits,
            None,
        )
        .await
        .map_err(|error| SwarmError::WorktreeIo(format!("isolated Git command failed: {error}")))?;
        if !output.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "isolated Git command failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    async fn checkout_git_stdout(&self, checkout: &Path, args: &[&str]) -> Result<String> {
        let output = capture_bounded_process(
            self.checkout_git_command(checkout, args),
            self.capture_limits,
            None,
        )
        .await
        .map_err(|error| SwarmError::WorktreeIo(format!("isolated Git command failed: {error}")))?;
        if !output.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "isolated Git command failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn validate_swarm_root(&self) -> Result<()> {
        ensure_unchanged_real_directory(&self.swarm_root, &self.swarm_parent)
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
