//! Worktree accounting, cleanup, and Git command execution.

use super::*;

impl WorktreeManager {
    pub(super) async fn transfer_closure_bytes(&self, pinned_head: &str) -> Result<u64> {
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

    pub(super) async fn checkout_logical_bytes(&self, pinned_head: &str) -> Result<u64> {
        let output = capture_bounded_process(
            self.git_command(&["ls-tree", "-rl", "-r", "-z", pinned_head, "--"]),
            self.capture_limits,
            None,
        )
        .await
        .map_err(|error| capture_error("git checkout size measurement", error))?;
        if !output.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "git checkout size measurement failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let mut total = 0_u64;
        for entry in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let header = entry
                .split(|byte| *byte == b'\t')
                .next()
                .ok_or_else(|| SwarmError::WorktreeIo("invalid git ls-tree entry".to_owned()))?;
            let header = std::str::from_utf8(header)
                .map_err(|_| SwarmError::WorktreeIo("non-UTF-8 git ls-tree header".to_owned()))?;
            let size = header.split_whitespace().nth(3).ok_or_else(|| {
                SwarmError::WorktreeIo("git ls-tree entry omitted object size".to_owned())
            })?;
            if size == "-" {
                continue;
            }
            total = total
                .checked_add(size.parse::<u64>().map_err(|_| {
                    SwarmError::WorktreeIo("git ls-tree returned invalid object size".to_owned())
                })?)
                .ok_or_else(|| SwarmError::WorktreeIo("checkout size overflowed".to_owned()))?;
        }
        Ok(total)
    }

    pub(super) fn reserved_workspace_bytes(&self) -> Result<u64> {
        let mut total = 0_u64;
        for entry in std::fs::read_dir(&self.swarm_root)? {
            let entry = entry?;
            // Name equality, not full-path equality: every `read_dir` entry here
            // is by construction a DIRECT child of `swarm_root`, and the control
            // directory is by construction that root's child named by the
            // constant, so the two tests are exactly equivalent — including for a
            // hostile entry literally carrying that name, which the previous code
            // also skipped. Keying on the name makes the test independent of the
            // path representation, so a future re-split cannot silently reopen
            // the miscount. Never canonicalize an entry inside this loop: that
            // would resolve a planted symlink BEFORE `is_real_directory_entry`
            // gets to refuse it.
            if entry.file_name().as_os_str() == std::ffi::OsStr::new(CONTROL_DIR) {
                continue;
            }
            if !is_real_directory_entry(&entry.path())? {
                return Err(SwarmError::WorktreeIo(format!(
                    "refused linked workspace reservation entry: {}",
                    entry.path().display()
                )));
            }
            let reservation = entry.path().join(RESERVATION_FILE);
            let root_authority = DirectoryAuthority::open(&entry.path())?;
            let bytes = if transaction_is_active(&root_authority, &entry.path())? {
                let owner = entry.file_name().into_string().map_err(|_| {
                    SwarmError::DispatchAdmission(
                        "active reservation owner is not valid UTF-8".to_owned(),
                    )
                })?;
                let retained = self
                    .active_reservations
                    .lock()
                    .map_err(|_| {
                        SwarmError::WorktreeIo("active reservation registry is poisoned".to_owned())
                    })?
                    .get(&owner)
                    .cloned();
                if let Some(retained) = retained {
                    if root_authority.identity_token() != retained.root_identity {
                        return Err(SwarmError::DispatchAdmission(
                            "active reservation root identity changed".to_owned(),
                        ));
                    }
                    validate_reservation_authority(
                        &retained.authority,
                        &reservation,
                        retained.bytes,
                    )?;
                    retained.bytes
                } else {
                    // A foreign or recovered active transaction has no held
                    // receipt in this manager. Count the full ceiling rather
                    // than trust unauthenticated mutable same-path bytes.
                    MAX_TRANSACTION_WORKSPACE_BYTES
                }
            } else {
                match std::fs::symlink_metadata(&reservation) {
                    Ok(_) => read_workspace_reservation(&reservation)?,
                    Err(error) if error.kind() == ErrorKind::NotFound => {
                        // Legacy worktrees and foreign retained evidence
                        // predate reservation receipts. Count either at the
                        // transaction ceiling so compatibility cannot
                        // understate authority.
                        MAX_TRANSACTION_WORKSPACE_BYTES
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            total = total.checked_add(bytes).ok_or_else(|| {
                SwarmError::WorktreeIo("aggregate workspace reservation overflow".to_owned())
            })?;
        }
        Ok(total)
    }

    fn remove_owned_transaction_root(
        &self,
        owner: &str,
        root: &Path,
        root_authority: &DirectoryAuthority,
    ) -> Result<()> {
        self.validate_swarm_root()?;
        remove_transaction_root(
            &self.swarm_root,
            &self.swarm_authority,
            owner,
            root,
            root_authority.clone(),
            &self.control_root,
            &DirectoryAuthority::open(&self.control_root)?,
        )
        .map_err(|(error, _)| error)
    }

    /// Release a completed transaction and its persisted capacity reservation.
    pub fn release_transaction(&self, workspace: &TransactionWorkspace) -> Result<()> {
        workspace.cleanup.release()
    }

    /// Reclaim transaction roots left behind by a process that no longer
    /// exists, and NOTHING else. Returns how many were reclaimed.
    ///
    /// # The defect this closes (F-2)
    ///
    /// An uncatchable kill leaves every in-flight transaction root on disk with
    /// its reservation receipt intact. `reserved_workspace_bytes` keeps counting
    /// those receipts, so the next dispatch sums the dead run's reservations
    /// against a budget nobody is using and refuses with *"dispatch aggregate
    /// workspace budget is already committed"*. Measured on Linux: eight workers
    /// killed with `kill -9 -<PGID>` left eight orphaned roots, and every later
    /// dispatch on that repository was refused until a human deleted
    /// `.swarm-worktrees/` by hand. `cleanup_all` could have reclaimed them, but
    /// nothing invoked it on a fresh process at dispatch time — so the product
    /// survived the crash and then could not be restarted, which reads as
    /// correct fail-closed behaviour while actually being unrecoverable.
    ///
    /// # Why this cannot reclaim a budget a live run legitimately holds
    ///
    /// The discriminator is the transaction's own lease, and it is enforced by
    /// the KERNEL rather than inferred:
    ///
    /// * a live owner holds an exclusive `flock` on `<root>/.wayland-active-lease`
    ///   for the whole life of its transaction, taken inside the same swarm
    ///   critical section that creates the root. [`transaction_is_active`]
    ///   observes that as contention and this skips the root, leaving its
    ///   reservation committed;
    /// * that lock is released ONLY when the holding process exits — by exit,
    ///   by signal, or by `kill -9`. There is no timeout, no heartbeat and no
    ///   age heuristic here, so a slow but live peer can never be mistaken for a
    ///   dead one however long it runs;
    /// * `flock` is per open file description, not per process, so a sibling
    ///   transaction in THIS process is observed as held exactly like one in
    ///   another process;
    /// * only a root carrying a parseable reservation receipt is a candidate at
    ///   all, so legacy linked worktrees and foreign retained evidence keep
    ///   being counted and are left to `cleanup_all`, which knows to prune the
    ///   parent repository's worktree metadata rather than unlink a checkout
    ///   under it;
    /// * this manager's own in-flight owners are skipped a second time via
    ///   `active_reservations`, which `TransactionCleanup::release` only clears
    ///   while holding the sentinel this function holds;
    /// * the whole scan runs under that sentinel, so it cannot interleave with
    ///   an admission that has created a root but not yet taken its lease.
    ///
    /// A root that resists removal is LEFT counted rather than dropped from the
    /// accounting: reclaim failures are reported, never assumed. The result is
    /// that a failure to reclaim degrades to the pre-existing refusal, which is
    /// the safe direction.
    pub fn reclaim_abandoned_transactions(&self) -> Result<usize> {
        self.validate_swarm_root()?;
        with_directory_lock(&self.swarm_root, &self.swarm_authority, || {
            let mut reclaimed = 0_usize;
            for entry in std::fs::read_dir(&self.swarm_root)? {
                let entry = entry?;
                // Name equality, never path equality or canonicalization: see
                // `reserved_workspace_bytes`.
                let name = entry.file_name();
                if name.as_os_str() == std::ffi::OsStr::new(CONTROL_DIR) {
                    continue;
                }
                let path = entry.path();
                if !is_real_directory_entry(&path)? {
                    // A non-directory child is not a transaction root. Leave it
                    // for `cleanup_all`, which refuses it explicitly.
                    continue;
                }
                let Ok(owner) = name.into_string() else {
                    continue;
                };
                if self
                    .active_reservations
                    .lock()
                    .map_err(|_| {
                        SwarmError::WorktreeIo("active reservation registry is poisoned".to_owned())
                    })?
                    .contains_key(&owner)
                {
                    continue;
                }
                // Reclaim ONLY roots this manager's own transaction path minted,
                // proven by a parseable reservation receipt. A root without one
                // is a legacy linked worktree or foreign retained evidence: it
                // is still counted at the ceiling by
                // `reserved_workspace_bytes`, but removing it belongs to
                // `cleanup_all`, which runs `git worktree remove` and prunes the
                // parent repository's administrative metadata. Tearing such a
                // root out with a plain recursive delete would strand that
                // metadata, and reclaiming residue this function cannot
                // identify is not what F-2 asks for.
                if read_workspace_reservation(&path.join(RESERVATION_FILE)).is_err() {
                    continue;
                }
                let root_authority = DirectoryAuthority::open(&path)?;
                if transaction_is_active(&root_authority, &path)? {
                    continue;
                }
                match self.remove_owned_transaction_root(&owner, &path, &root_authority) {
                    Ok(()) => reclaimed += 1,
                    Err(error) => tracing::warn!(
                        path = %path.display(),
                        error = %error,
                        "abandoned transaction root could not be reclaimed; \
                         its reservation stays counted against the dispatch budget"
                    ),
                }
            }
            Ok(reclaimed)
        })
    }

    /// Remove every directory under `.swarm-worktrees/` via
    /// `git worktree remove --force`. Attempts every safe entry, then reports
    /// all failures and every residual path. Idempotent when the root is empty.
    pub async fn cleanup_all(&self, escalation: &CancellationToken) -> Result<()> {
        self.validate_swarm_root()?;
        // The SAME sentinel `with_directory_lock` takes, so full cleanup still
        // serializes against every admission critical section. This lock is on
        // the SWARM authority; the two checkout-registration leases are on their
        // TRANSACTION-ROOT authorities. Collapsing them onto one sentinel would
        // deadlock cleanup against the transaction it is trying to remove.
        let _cleanup_lock = ActiveLease::acquire(swarm_lock_handle(&self.swarm_authority)?)?;
        let deadline = tokio::time::Instant::now() + CLEANUP_GRACE;
        let mut failures = Vec::new();
        let mut entries = Vec::new();
        match std::fs::symlink_metadata(&self.swarm_root) {
            Ok(_) => {
                self.validate_swarm_root()?;
                for entry in std::fs::read_dir(&self.swarm_root)? {
                    match entry {
                        Ok(entry) => {
                            // Name equality, never path equality or
                            // canonicalization: see `reserved_workspace_bytes`.
                            let name = entry.file_name();
                            if name.as_os_str() == std::ffi::OsStr::new(CONTROL_DIR) {
                                continue;
                            }
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
            let root_authority = match DirectoryAuthority::open(&path) {
                Ok(authority) => authority,
                Err(error) => {
                    failures.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            match transaction_is_active(&root_authority, &path) {
                Ok(true) => {
                    failures.push(format!("active transaction preserved: {}", path.display()));
                    continue;
                }
                Ok(false) => {}
                Err(error) => {
                    failures.push(format!("{}: {error}", path.display()));
                    continue;
                }
            }
            let reservation = path.join(RESERVATION_FILE);
            match read_workspace_reservation(&reservation) {
                Ok(_) => {
                    let owner =
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .ok_or_else(|| {
                                SwarmError::WorktreeIo(format!(
                                    "transaction workspace has invalid owner path: {}",
                                    path.display()
                                ))
                            })?;
                    match self.remove_owned_transaction_root(owner, &path, &root_authority) {
                        Ok(()) => continue,
                        Err(error) => {
                            failures.push(format!("{}: {error}", path.display()));
                            continue;
                        }
                    }
                }
                Err(error) if is_legacy_linked_worktree(&path) => {
                    tracing::debug!(
                        path = %path.display(),
                        error = %error,
                        "cleaning legacy linked worktree without reservation authority"
                    );
                }
                Err(error) => {
                    failures.push(format!("{}: {error}", path.display()));
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
                // Name equality, never path equality or canonicalization: see
                // `reserved_workspace_bytes`.
                Ok(entry) if entry.file_name().as_os_str() == std::ffi::OsStr::new(CONTROL_DIR) => {
                }
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

    // Gated to Linux, not to `unix`, because every call site lives in
    // `worktree_tests/linux.rs`, which is itself `cfg(target_os = "linux")`.
    // Under a plain `unix` gate this compiles on macOS with no callers, and
    // `-D warnings` turns the resulting dead_code lint into a CI failure.
    // Matches the `set_ambient_git_env` sibling below, gated the same way for
    // the same reason.
    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn new_with_git_script_and_limits(
        repo_root: &Path,
        script: &str,
        capture_limits: CaptureLimits,
    ) -> Result<Self> {
        let mut manager = Self::new(repo_root)?;
        manager.git_program = "/bin/sh".to_owned();
        manager.git_prefix_args = vec!["-c".to_owned(), script.to_owned(), "--".to_owned()];
        manager.capture_limits = capture_limits;
        Ok(manager)
    }

    /// Gated to the configuration that compiles its callers, NOT deleted. All
    /// four call sites live in `worktree_tests/linux.rs`, which is itself
    /// `cfg(target_os = "linux")`, so this is dead on Windows and very much
    /// alive on Linux — removing it would break the Linux test build. The gate
    /// mirrors the sibling `new_with_git_script_and_limits` above, which uses
    /// the same `cfg(all(test, ..))` shape for the same reason.
    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn set_ambient_git_env(&mut self, key: &str, value: impl Into<std::ffi::OsString>) {
        self.ambient_git_env.push((key.to_string(), value.into()));
    }

    pub(super) async fn reject_executable_checkout_config(&self) -> Result<()> {
        self.validate_repo_authority()?;
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

    pub(super) fn git_command(&self, args: &[&str]) -> tokio::process::Command {
        let hooks = format!("core.hooksPath={}", self.disabled_hooks.display());
        let mut protected_args = Vec::new();
        #[cfg(test)]
        protected_args.extend(self.git_prefix_args.iter().cloned());
        protected_args.extend([
            "-c".to_string(),
            hooks,
            "-c".to_string(),
            "core.fsmonitor=false".to_string(),
            "-c".to_string(),
            "core.autocrlf=false".to_string(),
        ]);
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

    pub(super) async fn run_checkout_git(&self, checkout: &Path, args: &[&str]) -> Result<()> {
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

    pub(super) async fn checkout_git_stdout(
        &self,
        checkout: &Path,
        args: &[&str],
    ) -> Result<String> {
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

    pub(super) fn validate_swarm_root(&self) -> Result<()> {
        self.validate_repo_authority()?;
        self.swarm_authority.validate_path(&self.swarm_root)?;
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
