//! Worktree construction and capacity admission.

use super::*;

impl WorktreeManager {
    /// Construct a new manager for `repo_root`. Creates the
    /// `.swarm-worktrees/` directory if it does not exist.
    pub fn new(repo_root: &Path) -> Result<Self> {
        // De-verbatimize the canonicalized roots: on Windows a bare
        // `std::fs::canonicalize` returns a `\\?\C:\...` verbatim path, which the
        // PowerShell capacity probe (and downstream git invocations that inherit
        // the root) cannot consume. The shared helper strips the `\\?\` prefix for
        // real drive-letter paths and is a no-op on unix and for genuine
        // UNC/device paths, so Linux is unaffected.
        let repo_root = normalized_root(repo_root)?;
        let repo_authority = DirectoryAuthority::open_observational(&repo_root)?;
        let swarm_root = repo_root.join(SWARM_ROOT_DIR);
        ensure_real_directory(&swarm_root)?;
        let swarm_root = normalized_root(&swarm_root)?;
        let swarm_authority = DirectoryAuthority::open(&swarm_root)?;
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
        let control_root = swarm_root.join(CONTROL_DIR);
        ensure_real_directory(&control_root)?;
        make_guard_dir_private(&control_root)?;
        let control_root = normalized_root(&control_root)?;
        Ok(Self {
            swarm_parent: repo_root.clone(),
            repo_root,
            repo_authority,
            swarm_root,
            swarm_authority,
            git_program: "git".to_string(),
            capture_limits: GIT_CAPTURE_LIMITS,
            _git_guard_dir: git_guard_dir,
            empty_git_config,
            disabled_hooks,
            control_root,
            admission_lock: Mutex::new(()),
            active_reservations: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            ambient_git_env: Vec::new(),
            #[cfg(test)]
            git_prefix_args: Vec::new(),
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
        let workspace_parent = workspace_root.parent().ok_or_else(|| {
            SwarmError::WorktreeIo("orchestrator worktree root has no parent".to_owned())
        })?;
        std::fs::create_dir_all(workspace_parent)?;
        // De-verbatimize every canonicalized root, exactly as `new` does: on
        // Windows a bare `std::fs::canonicalize` returns a `\\?\C:\...` verbatim
        // path, and a verbatim `swarm_root` reaches the PowerShell capacity probe
        // where `[IO.DriveInfo]::new` throws on the `\\?\C:\` drive root. The
        // shared helper strips the `\\?\` prefix for real drive-letter paths and
        // is a no-op on unix and for genuine UNC/device paths, so Linux is
        // unaffected. The invariant is NOT that these roots end up plain — the
        // strip is conditional — but that BOTH operands of every root comparison
        // are produced by that one derivation, including in
        // `new_with_workspace_authority`, which re-derives the inherited display
        // path through the same helper. The checks below therefore hold whether
        // or not the strip was actually applied.
        let workspace_parent = normalized_root(workspace_parent)?;
        let repo_root = normalized_root(repo_root)?;
        if workspace_parent.starts_with(&repo_root) || repo_root.starts_with(&workspace_parent) {
            return Err(SwarmError::WorktreeIo(format!(
                "orchestrator worktree root must not overlap repository {}",
                repo_root.display()
            )));
        }
        ensure_real_directory(workspace_root)?;
        make_guard_dir_private(workspace_root)?;
        let swarm_root = normalized_root(workspace_root)?;
        if swarm_root.parent() != Some(workspace_parent.as_path()) {
            return Err(SwarmError::WorktreeIo(format!(
                "refused worktree root outside orchestrator directory: {}",
                swarm_root.display()
            )));
        }
        let authority = wcore_sandbox::DirectoryAuthority::open(&swarm_root)
            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?;
        Self::new_with_workspace_authority(&repo_root, authority)
    }

    /// Construct a manager from an already-retained orchestrator workspace
    /// capability. This is the only constructor suitable for handing a
    /// transaction root across an authority boundary: it preserves the exact
    /// directory object instead of canonicalizing a display path and minting a
    /// replacement authority.
    pub fn new_with_workspace_authority(
        repo_root: &Path,
        workspace_authority: wcore_sandbox::DirectoryAuthority,
    ) -> Result<Self> {
        let repo_root = normalized_root(repo_root)?;
        let repo_authority = DirectoryAuthority::open_observational(&repo_root)?;
        let swarm_root = normalized_root(workspace_authority.display_path())?;
        if !swarm_root.is_absolute() {
            return Err(SwarmError::WorktreeIo(
                "orchestrator worktree capability must have an absolute display path".to_owned(),
            ));
        }
        let workspace_parent = swarm_root
            .parent()
            .ok_or_else(|| {
                SwarmError::WorktreeIo("orchestrator worktree root has no parent".to_owned())
            })?
            .to_path_buf();
        if workspace_parent.starts_with(&repo_root) || repo_root.starts_with(&workspace_parent) {
            return Err(SwarmError::WorktreeIo(format!(
                "orchestrator worktree root must not overlap repository {}",
                repo_root.display()
            )));
        }
        workspace_authority
            .validate_path(&swarm_root)
            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?;
        let swarm_authority = DirectoryAuthority::from_sandbox(workspace_authority);
        let git_guard_dir = tempfile::Builder::new()
            .prefix("wayland-swarm-git-")
            .tempdir()?;
        make_guard_dir_private(git_guard_dir.path())?;
        let empty_git_config = git_guard_dir.path().join("empty-gitconfig");
        write_empty_private_config(&empty_git_config)?;
        let disabled_hooks = git_guard_dir.path().join("disabled-hooks");
        let control_authority = swarm_authority.open_or_create_child_directory(CONTROL_DIR)?;
        let control_root = swarm_root.join(CONTROL_DIR);
        control_authority.validate_path(&control_root)?;
        let control_root = normalized_root(&control_root)?;
        Ok(Self {
            repo_root,
            repo_authority,
            swarm_root,
            swarm_authority,
            swarm_parent: workspace_parent,
            git_program: "git".to_string(),
            capture_limits: GIT_CAPTURE_LIMITS,
            _git_guard_dir: git_guard_dir,
            empty_git_config,
            disabled_hooks,
            control_root,
            admission_lock: Mutex::new(()),
            active_reservations: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            ambient_git_env: Vec::new(),
            #[cfg(test)]
            git_prefix_args: Vec::new(),
        })
    }

    /// Return the underlying repository root.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Revalidate that the pathname still names the repository directory held
    /// when this manager was constructed.
    ///
    /// This function is the ONLY consumer of `repo_authority`: the repository
    /// authority is an identity witness and never mutates, deletes, renames or
    /// creates beneath the root. It is therefore acquired observationally, and
    /// that is load-bearing rather than incidental — every manager git
    /// invocation sets `current_dir(&self.repo_root)` (see
    /// `worktree_cleanup::git_command`), and on Windows a retained handle
    /// carrying the DELETE access right blocks the in-process chdir that
    /// NEED_WORK_TREE subcommands such as `status` perform inside
    /// `setup_work_tree()`. Reintroducing the mutating open here fails 10
    /// Windows tests, starting with `assert_clean`, which gates dispatch,
    /// isolated checkout and integration checkout alike.
    pub(crate) fn validate_repo_authority(&self) -> Result<()> {
        self.repo_authority.validate_path(&self.repo_root)
    }

    /// Return the swarm worktree root (`<repo>/.swarm-worktrees/`).
    pub fn swarm_root(&self) -> &Path {
        &self.swarm_root
    }

    pub(crate) async fn sandbox_read_denies(
        &self,
        workspace: &TransactionWorkspace,
    ) -> Result<Vec<PathBuf>> {
        self.validate_swarm_root()?;
        let mut denied = vec![
            self.git_common_dir().await?,
            self.control_root.clone(),
            workspace.root.join(RESERVATION_FILE),
            workspace.root.join(LEASE_FILE),
        ];
        // Both the worker's own transaction root and the control directory are
        // direct children of `swarm_root`, so compare child NAMES: a full-path
        // test denied the worker its OWN root while its checkout sat in
        // fs_read_allow. Fail closed if the root has no name to compare.
        let workspace_name = workspace.root.file_name().ok_or_else(|| {
            SwarmError::WorktreeIo("transaction root has no directory name".to_owned())
        })?;
        for entry in std::fs::read_dir(&self.swarm_root)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.as_os_str() != workspace_name
                && name.as_os_str() != std::ffi::OsStr::new(CONTROL_DIR)
            {
                denied.push(entry.path());
            }
        }
        Ok(denied)
    }

    /// Count retained worker worktrees without following linked entries.
    /// Enumeration stops once `stop_after` is exceeded so an already-invalid
    /// worktree root cannot force an unbounded admission scan.
    pub fn retained_worker_count(&self, stop_after: usize) -> Result<usize> {
        self.validate_repo_authority()?;
        self.validate_swarm_root()?;
        let mut count = 0_usize;
        for entry in std::fs::read_dir(&self.swarm_root)? {
            let entry = entry?;
            // Name equality, never path equality or canonicalization: see `reserved_workspace_bytes`.
            if entry.file_name().as_os_str() == std::ffi::OsStr::new(CONTROL_DIR) {
                continue;
            }
            let path = entry.path();
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
    ///
    /// The result is deliberately NOT routed through the shared path helper.
    /// This is the one swarm-exported path `wcore-agent/src/spawner.rs` consumes
    /// WITHOUT re-canonicalizing (`:1557`), comparing it there against spawner's
    /// own verbatim `workspace_root` to build the deny-root overlap evidence
    /// (`:1385-1390`). Normalizing it would render the two operands differently
    /// and make both `starts_with` directions of that check unconditionally
    /// false — a new fail-open of exactly the class the shared representation
    /// closes. The exclusion is principled rather than an omission: this path is
    /// never stored on the manager, it is freshly resolved and exported.
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

    /// The parent repo's current branch — the short symbolic HEAD (e.g. `main`).
    ///
    /// A delegated landing advances THIS branch inside the Wayland-owned
    /// integration clone (surface-for-accept): the landed successor lands on the
    /// clone's `refs/heads/<branch>`, never touching the user's working tree.
    /// Fails closed on a detached HEAD (no branch to land onto) and on an
    /// option-like name (defense-in-depth, since the name flows into a
    /// `git clone --branch <name>` argv).
    pub async fn current_branch(&self) -> Result<String> {
        self.validate_repo_authority()?;
        let cmd = self.git_command(&["symbolic-ref", "--quiet", "--short", "HEAD"]);
        let out = capture_bounded_process(cmd, self.capture_limits, None)
            .await
            .map_err(|error| capture_error("git symbolic-ref HEAD", error))?;
        if !out.status.success() {
            return Err(SwarmError::WorktreeIo(
                "git symbolic-ref HEAD failed: a detached HEAD has no branch to land onto"
                    .to_owned(),
            ));
        }
        let branch = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if branch.is_empty() {
            return Err(SwarmError::WorktreeIo(
                "git symbolic-ref HEAD returned an empty branch name".to_owned(),
            ));
        }
        reject_option_like_ref("branch", &branch)?;
        Ok(branch)
    }

    /// Resolve `base` once and require it to name the current clean checkout.
    ///
    /// Standalone delegated checkouts are constructed from the parent's exact
    /// HEAD object. Accepting a different symbolic base here would make the
    /// public brief claim one authority while the transaction receives
    /// another, so that mismatch fails before any workspace is materialized.
    pub async fn pinned_dispatch_base(&self, base: &str) -> Result<String> {
        reject_option_like_ref("base", base)?;
        self.reject_executable_checkout_config().await?;
        let requested = format!("{base}^{{commit}}");
        let output = capture_bounded_process(
            self.git_command(&["rev-parse", "--verify", requested.as_str()]),
            self.capture_limits,
            None,
        )
        .await
        .map_err(|error| capture_error("git rev-parse dispatch base", error))?;
        if !output.status.success() {
            return Err(SwarmError::WorktreeIo(format!(
                "git rev-parse dispatch base failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let resolved = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let head = self.read_pinned_head().await?;
        if resolved != head {
            return Err(SwarmError::DispatchAdmission(format!(
                "requested base {base} resolves to {resolved}, but the clean dispatch checkout is pinned at {head}"
            )));
        }
        Ok(head)
    }

    /// Prove usable storage and return the bounded workspace authority used by
    /// every transaction in one admitted dispatch.
    ///
    /// The two counts are deliberately separate because they answer different
    /// questions and are NOT interchangeable:
    ///
    /// * `active_workers` divides the aggregate budget into the per-transaction
    ///   ceiling. It is the fair-share denominator, so a fuller roster yields a
    ///   smaller ceiling.
    /// * `admitted_transactions` is how many NEW transaction roots this call
    ///   admits. Only those are charged against free space here, because every
    ///   workspace that already exists is already counted — by name, at its own
    ///   receipt — in `reserved_workspace_bytes()`.
    ///
    /// Charging `active_workers` new ceilings double-counts the roster: the
    /// `wcore-agent` spawner admits exactly one child while passing
    /// `retained + 1`, so at a near-full evidence roster it demanded
    /// 256 * 256 MiB + 512 MiB = 64.5 GiB of free space to create one small
    /// checkout, and no host under that could ever admit a child. The flat
    /// per-transaction ceiling itself is intentional and stays: a workspace is
    /// allowed to GROW to it after admission, so reserving the measured initial
    /// checkout size instead would under-reserve by construction.
    pub async fn workspace_capacity(
        &self,
        active_workers: usize,
        admitted_transactions: usize,
    ) -> Result<WorkspaceCapacity> {
        self.validate_repo_authority()?;
        self.validate_swarm_root()?;
        let active_workers = u64::try_from(active_workers.max(1)).map_err(|_| {
            SwarmError::DispatchAdmission("active worker count exceeds u64".to_owned())
        })?;
        let admitted_transactions = u64::try_from(admitted_transactions.max(1)).map_err(|_| {
            SwarmError::DispatchAdmission("admitted transaction count exceeds u64".to_owned())
        })?;
        let available_bytes = self.available_workspace_bytes().await?;
        let max_transaction_bytes = per_transaction_ceiling(active_workers);
        let existing_reservation = self.reserved_workspace_bytes()?;
        let (admitted_reservation, active_reservation) = admission_reservation(
            max_transaction_bytes,
            admitted_transactions,
            existing_reservation,
        )
        .ok_or_else(|| {
            SwarmError::DispatchAdmission("dispatch workspace capacity overflowed".to_owned())
        })?;
        if active_reservation > available_bytes {
            return Err(SwarmError::DispatchAdmission(format!(
                "dispatch requires {active_reservation} bytes for {admitted_transactions} new transaction(s) sized for {active_workers} active workers, {existing_reservation} bytes already reserved, and its safety margin, but only {available_bytes} bytes are available"
            )));
        }
        if existing_reservation
            .checked_add(admitted_reservation)
            .is_none_or(|total| total > MAX_AGGREGATE_WORKSPACE_BYTES)
        {
            return Err(SwarmError::DispatchAdmission(
                "dispatch aggregate workspace budget is already committed".to_owned(),
            ));
        }
        Ok(WorkspaceCapacity {
            available_bytes,
            safety_margin_bytes: WORKSPACE_SAFETY_MARGIN_BYTES,
            max_transaction_bytes,
            max_aggregate_bytes: MAX_AGGREGATE_WORKSPACE_BYTES,
        })
    }

    /// Reject an entire dispatch before any transaction is created when the
    /// pinned repository cannot fit the per-active-worker budget.
    pub(crate) async fn assert_dispatch_checkout_fits(
        &self,
        pinned_head: &str,
        capacity: WorkspaceCapacity,
    ) -> Result<()> {
        let closure_bytes = self.transfer_closure_bytes(pinned_head).await?;
        let checkout_bytes = self.checkout_logical_bytes(pinned_head).await?;
        let initial_bytes = closure_bytes.checked_add(checkout_bytes).ok_or_else(|| {
            SwarmError::WorktreeIo("initial workspace size overflowed".to_owned())
        })?;
        if initial_bytes > capacity.max_transaction_bytes {
            return Err(SwarmError::DispatchAdmission(format!(
                "dispatch checkout requires {initial_bytes} initial bytes ({closure_bytes} Git object bytes plus {checkout_bytes} logical checkout bytes), exceeding the {}-byte per-worker budget before any worker was created",
                capacity.max_transaction_bytes
            )));
        }
        Ok(())
    }

    #[cfg(unix)]
    async fn available_workspace_bytes(&self) -> Result<u64> {
        let root = self.swarm_root.to_string_lossy().into_owned();
        let output = capture_bounded_process(
            shell::shell_command_argv("df", &["-Pk", root.as_str()]),
            self.capture_limits,
            None,
        )
        .await
        .map_err(|error| capture_error("workspace capacity probe", error))?;
        if !output.status.success() {
            return Err(SwarmError::DispatchAdmission(format!(
                "workspace capacity probe failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let line = String::from_utf8_lossy(&output.stdout)
            .lines()
            .last()
            .ok_or_else(|| {
                SwarmError::DispatchAdmission("workspace capacity probe returned no data".into())
            })?
            .to_owned();
        let blocks = line
            .split_whitespace()
            .nth(3)
            .ok_or_else(|| {
                SwarmError::DispatchAdmission(
                    "workspace capacity probe omitted available blocks".into(),
                )
            })?
            .parse::<u64>()
            .map_err(|_| {
                SwarmError::DispatchAdmission(
                    "workspace capacity probe returned invalid available blocks".into(),
                )
            })?;
        blocks.checked_mul(1024).ok_or_else(|| {
            SwarmError::DispatchAdmission("workspace capacity probe overflowed".into())
        })
    }

    /// Derive the drive root (e.g. `C:\`) the capacity probe needs from the
    /// stored workspace root's own `Prefix` component, in Rust rather than by
    /// asking PowerShell to parse the transported path. The de-verbatimizing
    /// strip is CONDITIONAL, so a long, reserved-name or non-UTF-8 workspace path
    /// can legitimately still be verbatim; reading the prefix accepts both forms.
    /// A root with no drive-letter prefix (a UNC or device path, where
    /// `DriveInfo` does not apply) fails CLOSED with a legible admission error.
    #[cfg(windows)]
    fn probe_drive_root(&self) -> Result<String> {
        use std::path::{Component, Prefix};

        let letter = match self.swarm_root.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
                _ => {
                    return Err(SwarmError::DispatchAdmission(
                        "workspace capacity probe requires a drive-letter workspace root".into(),
                    ));
                }
            },
            _ => {
                return Err(SwarmError::DispatchAdmission(
                    "workspace capacity probe requires a drive-letter workspace root".into(),
                ));
            }
        };
        Ok(format!("{}:\\", (letter as char).to_ascii_uppercase()))
    }

    #[cfg(windows)]
    async fn available_workspace_bytes(&self) -> Result<u64> {
        // Transport the probe root out-of-band via an environment variable rather
        // than as a trailing `-Command` argument. PowerShell re-parses trailing
        // args after `-Command` as script text, so any weird path character (a
        // `\\?\` verbatim prefix, `&`, `(`, spaces) breaks the probe with a
        // ParserError. Reading `$env:WCORE_SWARM_PROBE_ROOT` removes that reparse
        // class entirely. The probe no longer depends on the stored root's
        // rendering at all: `probe_drive_root` derives the drive root from that
        // path's own `Prefix` component and fails closed on a non-drive-letter
        // root, so `[IO.DriveInfo]::new` always receives a plain `C:\` rather than
        // a `\\?\C:\` verbatim path that would make it throw a non-terminating
        // `ArgumentException` (exit 0, empty stdout).
        const SCRIPT: &str = "$drive=[IO.DriveInfo]::new($env:WCORE_SWARM_PROBE_ROOT); [Console]::Out.Write($drive.AvailableFreeSpace)";
        let drive_root = self.probe_drive_root()?;
        let mut command = shell::shell_command_argv(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", SCRIPT],
        );
        command.env("WCORE_SWARM_PROBE_ROOT", drive_root.as_str());
        let output = capture_bounded_process(command, self.capture_limits, None)
            .await
            .map_err(|error| capture_error("workspace capacity probe", error))?;
        if !output.status.success() {
            return Err(SwarmError::DispatchAdmission(format!(
                "workspace capacity probe failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        // A PowerShell non-terminating error (e.g. `[IO.DriveInfo]::new` throwing
        // an ArgumentException) yields exit 0 with empty stdout, which would
        // otherwise reach `parse::<u64>()` and masquerade as "invalid bytes".
        // Treat empty output as a distinct probe failure so the real cause is
        // legible rather than mislabeled as an unparseable value.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return Err(SwarmError::DispatchAdmission(
                "workspace capacity probe produced no output".into(),
            ));
        }
        trimmed.parse::<u64>().map_err(|_| {
            SwarmError::DispatchAdmission(
                "workspace capacity probe returned unparseable available bytes".into(),
            )
        })
    }

    #[cfg(not(any(unix, windows)))]
    async fn available_workspace_bytes(&self) -> Result<u64> {
        Err(SwarmError::DispatchAdmission(
            "workspace capacity cannot be proven on this platform".into(),
        ))
    }

    async fn read_pinned_head(&self) -> Result<String> {
        self.validate_repo_authority()?;
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
    ///
    /// # Why this one gate has a second pass, and no other dirtiness check does
    ///
    /// `repo_root` is the **user's own repository**, which Wayland does not mint
    /// and therefore cannot guarantee the on-disk representation of. Everywhere
    /// else the swarm judges a tree, that tree was created by
    /// [`Self::git_command`] itself — same scrub in, same scrub out — so a
    /// literal byte comparison is exactly right and is left untouched.
    ///
    /// Here it is not. [`Self::git_command`] sets `GIT_CONFIG_NOSYSTEM=1` and
    /// empties the system/global config to defeat hostile configuration; that
    /// also strips Git for Windows' system-level `core.autocrlf=true`, so git
    /// compares literal bytes. A repository the user cloned normally on Windows,
    /// with no normalizing `.gitattributes`, therefore has a CRLF working tree
    /// against an LF index and reads as modified on a pristine tree — dispatch
    /// refused, naming files the user never touched.
    ///
    /// The second pass adjudicates that, and only that:
    ///
    /// * it runs **only** when every reported entry is a plain tracked-file
    ///   modification. An untracked file, an addition, a deletion, a rename, a
    ///   copy, an unmerged path or a type change refuses immediately, exactly as
    ///   before — the relaxation cannot reach them;
    /// * it then re-asks git with `--ignore-cr-at-eol --quiet`, in both
    ///   directions (worktree-vs-index and index-vs-HEAD), and reads the exit
    ///   code. If **any** difference survives that — or git does not exit
    ///   cleanly — the checkout is dirty and is refused with the original
    ///   report;
    /// * every hostile-config defence is unchanged: the second pass runs through
    ///   the same [`Self::git_command`], so `GIT_CONFIG_NOSYSTEM`, the emptied
    ///   system/global config, `GIT_ATTR_NOSYSTEM`, the disabled hooks path, the
    ///   disabled fsmonitor and the forced `core.autocrlf=false` all still apply.
    ///   Nothing is read back from the user's configuration, so no hostile value
    ///   can steer the decision.
    ///
    /// **Accepted trade-off, stated explicitly:** a working tree whose *only*
    /// uncommitted change is line-ending flips is now judged clean, so a worker
    /// may be dispatched over it. That is the price of not refusing every normal
    /// Windows checkout, and it is strictly narrower than the alternatives
    /// considered: forcing `core.autocrlf=input` would newly false-positive on
    /// every repository that legitimately commits CRLF, and judging the tree
    /// under the user's own configuration would re-admit exactly the hostile
    /// config this scrub exists to exclude.
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
        let reported = self.without_own_swarm_root(stdout.trim());
        let reported = reported.as_str();
        if reported.is_empty() {
            return Ok(());
        }
        if self.reported_dirt_is_line_endings_only(reported).await? {
            return Ok(());
        }
        Err(SwarmError::DirtyCheckout(reported.to_string()))
    }

    /// Drop the ONE porcelain entry that reports the swarm's own minted
    /// worktree root, and nothing else.
    ///
    /// # The defect this closes
    ///
    /// [`Self::new`] creates `<repo>/.swarm-worktrees/` INSIDE the repository
    /// whose cleanliness [`Self::assert_clean`] judges. That directory is
    /// untracked, so `git status --porcelain` reports `?? .swarm-worktrees/` and
    /// the guard refused dispatch because of an artifact the swarm itself had
    /// just created. Measured on Linux at the pre-fix commit, against a
    /// throwaway repository with no `.gitignore`:
    ///
    /// * `--workers 8` ran **one** worker and refused the other seven, each with
    ///   `dirty checkout — refused dispatch: ?? .swarm-worktrees/`, while the CLI
    ///   still exited 0 — so the fanout silently had an effective parallelism of
    ///   one;
    /// * the SECOND dispatch against the same repository failed outright with
    ///   exit 1 and zero workers, because the directory survives cleanup. That is
    ///   the restart path, so a killed run could not be resumed at all.
    ///
    /// The unit suite did not catch either, because its fixture commits a
    /// `.gitignore` naming `.swarm-worktrees/` — a workaround the shipping
    /// product never performs for the user.
    ///
    /// # Why this is not a weakening of the guard
    ///
    /// The exclusion is deliberately the narrowest one that fixes it, and it
    /// FAILS CLOSED everywhere else:
    ///
    /// * it applies only when this manager minted its root inside the repository
    ///   (`swarm_parent == repo_root`). An orchestrator-owned root lives outside
    ///   the repository, so a `.swarm-worktrees/` seen there is foreign residue
    ///   and keeps refusing;
    /// * it matches only the exact untracked-directory entry for that one path.
    ///   A tracked modification, an addition, a deletion, a rename, an unmerged
    ///   state, or any other path — including anything nested that git chose to
    ///   report individually — is untouched and still refuses;
    /// * it drops at most that single line. Every other reported entry survives
    ///   into the refusal message unchanged.
    ///
    /// The v0.2.2 incident this gate exists for is a *worker* contaminating the
    /// user's tree. The swarm's own container directory is not user work, and
    /// admitting it cannot admit a worker's changes: those live one level deeper,
    /// inside per-worker checkouts that are separate repositories.
    fn without_own_swarm_root(&self, reported: &str) -> String {
        if self.swarm_parent != self.repo_root {
            return reported.to_owned();
        }
        if self.swarm_root.file_name().and_then(|name| name.to_str()) != Some(SWARM_ROOT_DIR) {
            return reported.to_owned();
        }
        // Porcelain v1 renders an untracked directory as `?? <path>/`. Git quotes
        // a path containing unusual bytes, but this one is a fixed ASCII literal
        // this crate mints itself, so the unquoted form is the only one it can
        // produce.
        let own = format!("?? {SWARM_ROOT_DIR}/");
        reported
            .lines()
            .filter(|line| *line != own)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Second pass for [`Self::assert_clean`]: is every reported entry a
    /// line-ending-only difference on an already-tracked file?
    ///
    /// FAILS CLOSED at every step. A status code that is not a plain
    /// modification, a git invocation that does not exit zero, or any surviving
    /// difference all return `false`, which keeps the checkout dirty.
    async fn reported_dirt_is_line_endings_only(&self, reported: &str) -> Result<bool> {
        // Porcelain v1 prefixes each entry with a two-character XY code. Only a
        // modification of a tracked file can be an end-of-line artifact:
        // `??` (untracked), `A`/`D`/`R`/`C`/`T` (add, delete, rename, copy,
        // type change), `!` (ignored) and any unmerged `U`/`AA`/`DD` state are
        // real dirt and must keep refusing.
        for line in reported.lines() {
            match line.as_bytes() {
                [b' ', b'M', ..] | [b'M', b' ', ..] | [b'M', b'M', ..] => {}
                _ => return Ok(false),
            }
        }
        // The predicate is `--quiet`'s EXIT CODE, not the contents of a
        // `--name-only` listing. Measured on both hosts: with an LF index and a
        // CRLF working tree, `git diff --ignore-cr-at-eol --name-only` prints
        // nothing under git 2.54.0.windows.1 but still prints the path under git
        // 2.43.0 on Linux — even though `--stat` is empty there, so no hunk
        // survives. Whether the ignore flags are folded back into the pathname
        // listing is a git-version detail; `--exit-code` (which `--quiet`
        // implies) reports the compared content on both, 0 for no surviving
        // difference and 1 for one. Anything other than exactly 0 — including a
        // git error — keeps the checkout dirty.
        for args in [
            ["diff", "--ignore-cr-at-eol", "--quiet"].as_slice(),
            ["diff", "--cached", "--ignore-cr-at-eol", "--quiet"].as_slice(),
        ] {
            let out = capture_bounded_process(self.git_command(args), self.capture_limits, None)
                .await
                .map_err(|error| capture_error("git diff", error))?;
            if out.status.code() != Some(0) {
                return Ok(false);
            }
        }
        Ok(true)
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
        // Every filesystem site on this path names itself (see `io_at`). This
        // is the enforcement point that keeps that true for sites added later:
        // a bare `SwarmError::Io` reaching a caller renders as `io: <errno>`
        // and nothing else — no path, no probe — which is precisely what made
        // the macOS ENOENT in wayland#1025 unmeasurable from its CI output.
        Box::pin(self.create_isolated_checkout_inner(worker_id, branch, pinned_head, capacity))
            .await
            .map_err(|error| match error {
                SwarmError::Io(error) => {
                    SwarmError::WorktreeIo(format!("isolated checkout for {worker_id}: {error}"))
                }
                other => other,
            })
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

        self.validate_repo_authority()?;
        self.reject_executable_checkout_config().await?;
        self.assert_clean().await?;
        self.validate_swarm_root()?;
        let _admission = self.admission_lock.lock().await;
        let closure_bytes = self.transfer_closure_bytes(pinned_head).await?;
        let checkout_bytes = self.checkout_logical_bytes(pinned_head).await?;
        let initial_bytes = closure_bytes.checked_add(checkout_bytes).ok_or_else(|| {
            SwarmError::WorktreeIo("initial workspace size overflowed".to_owned())
        })?;
        let reserved_bytes = capacity.max_transaction_bytes;
        let required = reserved_bytes
            .checked_add(capacity.safety_margin_bytes)
            .ok_or_else(|| SwarmError::WorktreeIo("workspace capacity overflow".to_owned()))?;
        if initial_bytes > capacity.max_transaction_bytes {
            return Err(SwarmError::DispatchAdmission(format!(
                "workspace requires {initial_bytes} initial bytes ({closure_bytes} Git object bytes plus {checkout_bytes} logical checkout bytes), exceeding transaction budget {}",
                capacity.max_transaction_bytes
            )));
        }
        let transaction_root = self.swarm_root.join(worker_id);
        let (checkout, scratch, cleanup) = with_directory_lock(
            &self.swarm_root,
            &self.swarm_authority,
            || {
                let aggregate = self.reserved_workspace_bytes()?;
                if aggregate
                    .checked_add(reserved_bytes)
                    .is_none_or(|total| total > capacity.max_aggregate_bytes)
                {
                    return Err(SwarmError::DispatchAdmission(
                        "aggregate workspace budget exhausted".to_owned(),
                    ));
                }
                if aggregate
                    .checked_add(required)
                    .is_none_or(|total| total > capacity.available_bytes)
                {
                    return Err(SwarmError::DispatchAdmission(format!(
                        "workspace requires {required} bytes with {aggregate} already reserved, but authority proved only {} available bytes",
                        capacity.available_bytes,
                    )));
                }
                ensure_absent_destination(&transaction_root)?;
                std::fs::create_dir(&transaction_root).map_err(|error| {
                    io_at("creation of the transaction root", &transaction_root, error)
                })?;
                let registration = (|| {
                    make_guard_dir_private(&transaction_root).map_err(|error| {
                        io_at(
                            "private-mode set on the transaction root",
                            &transaction_root,
                            error,
                        )
                    })?;
                    // One representation for everything derived below: the root
                    // authority, the checkout/scratch joins, the lease file and
                    // TransactionCleanup.root. The checkout authority is opened on
                    // a path from this same derivation, which is what makes
                    // RetainedWorkspaceAuthority::new's parent-equality proof hold.
                    let transaction_root = normalized_root(&transaction_root)?;
                    let root_authority = DirectoryAuthority::open(&transaction_root)?;
                    let reservation_authority = Arc::new(
                        root_authority
                            .to_sandbox()
                            .create_child_file(
                                RESERVATION_FILE,
                                reserved_bytes.to_string().as_bytes(),
                            )
                            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?,
                    );
                    let checkout = transaction_root.join("checkout");
                    let scratch = transaction_root.join("scratch");
                    std::fs::create_dir(&scratch)
                        .map_err(|error| io_at("creation of the scratch root", &scratch, error))?;
                    make_guard_dir_private(&scratch).map_err(|error| {
                        io_at("private-mode set on the scratch root", &scratch, error)
                    })?;
                    // The lease is taken on the LEASE_FILE that the transaction
                    // derivation opens-or-creates, never on the transaction-root
                    // DIRECTORY object: Windows byte-range locking is undefined
                    // on a directory handle. The separate create-and-drop of the
                    // lease file is gone because the lease handle now creates it.
                    // The loan is still accounted against this root's counter, so
                    // `remove_open_dir_all` keeps refusing while the lease is held.
                    let lease = ActiveLease::acquire(transaction_lease_handle(&root_authority)?)?;
                    let swarm_authority = DirectoryAuthority::open(&self.swarm_root)?;
                    let quarantine_authority = DirectoryAuthority::open(&self.control_root)?;
                    let root_identity = root_authority.identity_token();
                    let cleanup = Arc::new(TransactionCleanup {
                        owner: worker_id.to_owned(),
                        root: transaction_root.clone(),
                        root_authority: StdMutex::new(Some(root_authority)),
                        checkout_authority: std::sync::OnceLock::new(),
                        swarm_root: self.swarm_root.clone(),
                        swarm_authority,
                        quarantine_root: self.control_root.clone(),
                        quarantine_authority,
                        reservation_authority,
                        reserved_bytes,
                        active_reservations: Arc::clone(&self.active_reservations),
                        release_lock: StdMutex::new(()),
                        lease: StdMutex::new(Some(lease)),
                        released: AtomicBool::new(false),
                    });
                    self.active_reservations
                        .lock()
                        .map_err(|_| {
                            SwarmError::WorktreeIo(
                                "active reservation registry is poisoned".to_owned(),
                            )
                        })?
                        .insert(
                            worker_id.to_owned(),
                            ActiveReservation {
                                root_identity,
                                authority: Arc::clone(&cleanup.reservation_authority),
                                bytes: reserved_bytes,
                            },
                        );
                    Ok((checkout, scratch, cleanup))
                })();
                if registration.is_err() {
                    let _ = std::fs::remove_dir_all(&transaction_root);
                }
                registration
            },
        )?;
        // The clone boundary must still refer to the directory object retained
        // at construction, not a same-path replacement.
        self.validate_repo_authority()?;
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
                cleanup.release()?;
                return Err(SwarmError::WorktreeIo(format!(
                    "isolated git clone failed: {error}"
                )));
            }
        };
        if !clone.status.success() {
            cleanup.release()?;
            return Err(SwarmError::WorktreeIo(format!(
                "isolated git clone failed: {}",
                String::from_utf8_lossy(&clone.stderr).trim()
            )));
        }
        make_guard_dir_private(&checkout)
            .map_err(|error| io_at("private-mode set on the cloned checkout", &checkout, error))?;

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
        let common = normalized_root(&common)?;
        let checkout_root = normalized_root(&checkout)?;
        if !common.starts_with(&checkout_root) {
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
        let checkout = normalized_root(&checkout)?;
        let scratch = normalized_root(&scratch)?;
        let transaction_root = normalized_root(&transaction_root)?;
        if !checkout.starts_with(&transaction_root)
            || !scratch.starts_with(&transaction_root)
            || checkout.starts_with(&scratch)
            || scratch.starts_with(&checkout)
        {
            return Err(SwarmError::WorktreeIo(
                "transaction checkout and scratch roots are not disjoint".to_owned(),
            ));
        }
        let authorities = Arc::new(TransactionWorkspaceAuthorities {
            // workstream C branch (i) — OBSERVATIONAL OPEN. Measured on
            // SEANDESKTOP, not inherited. A retained DELETE right makes
            // `SetCurrentDirectory` fail with a sharing violation, and
            // `git -C <checkout>` chdirs IN-PROCESS (MSYS `chdir()`), so a
            // DELETE-bearing checkout authority made every checkout-scoped git
            // invocation fail with "cannot change to ...: Permission denied".
            //
            // Determination 2: representation is INCIDENTAL, not causal — with
            // NO handle held, `git -C` succeeded with BOTH the verbatim
            // `\\?\C:\...` and the plain drive-letter form. Determination 3:
            // the checkout authority's complete consumer set is
            // `heartbeat::read_status_authorized` (relative child read),
            // `logical_tree_bytes` (read-only enumeration), `validate_path`
            // (identity), `CandidateSeal::mint` (manifest read), the loan-count
            // check in `TransactionCleanup::release`, and four `display_path()`
            // reads — NOTHING deletes, renames, creates a child or flushes
            // through it; the transaction root's destruction runs through the
            // ROOT authority's own recursion. Determination 4: the traverse
            // falsifier was tested directly rather than inherited from 20-72 —
            // through an observational handle, relative child reads, directory
            // enumeration, relative directory opens, identity checks and nested
            // reads ALL succeed.
            //
            // ANTI-SWAP IS UNCHANGED: the handle stays CONTINUOUSLY held, so
            // identity still survives a rename and no re-resolution window is
            // opened — byte-for-byte the property it had before. Widening this
            // back to `open` reintroduces the chdir refusal for every git
            // invocation scoped to this checkout, INCLUDING those made by CHILD
            // processes: the sharing conflict is system-wide, so a handle held
            // in the manager process denies a worker process's chdir just as
            // effectively.
            checkout: DirectoryAuthority::open_observational(&checkout)?,
            scratch: DirectoryAuthority::open(&scratch)?,
            reservation: Arc::clone(&cleanup.reservation_authority),
        });
        cleanup.bind_checkout_authority(authorities.checkout.clone());
        let workspace = TransactionWorkspace {
            owner: worker_id.to_owned(),
            root: transaction_root,
            checkout,
            scratch,
            base_commit: pinned_head.to_owned(),
            head_commit,
            tree,
            reserved_bytes,
            authorities,
            cleanup,
        };
        let materialized_bytes = workspace.logical_used_bytes()?;
        if materialized_bytes > workspace.reserved_bytes {
            let reserved_bytes = workspace.reserved_bytes;
            workspace.cleanup.release()?;
            return Err(SwarmError::DispatchAdmission(format!(
                "materialized workspace uses {materialized_bytes} bytes, exceeding transaction budget {reserved_bytes}"
            )));
        }
        Ok(workspace)
    }

    /// Create a Wayland-owned standalone integration checkout that the parent
    /// landing primitive ([`bind_integration_checkout`]) accepts as a durable
    /// landing target.
    ///
    /// Unlike [`create_isolated_checkout`], which mints a private *successor*
    /// working tree on a fresh branch, this produces the *target* main working
    /// tree: it clones `branch` from the source repository at `expected_head`
    /// through the identical `git clone --no-local --no-hardlinks` path, so the
    /// result is a main checkout with its own object store, an in-tree `.git`
    /// (`git_dir == common_git_dir`), no alternate object store, and is not a
    /// linked worktree — exactly the shape `bind_integration_checkout` requires.
    /// The target branch is checked out (non-detached, clean status) at the
    /// requested tip, and the returned [`TransactionWorkspace`] retains the
    /// checkout [`DirectoryAuthority`] plus the reservation/cleanup plumbing the
    /// landing path relies on. Fails closed on any clone, checkout, or invariant
    /// error.
    ///
    /// [`bind_integration_checkout`]: crate::worktree::WorktreeManager
    /// [`create_isolated_checkout`]: Self::create_isolated_checkout
    /// [`DirectoryAuthority`]: wcore_sandbox::DirectoryAuthority
    pub async fn create_integration_checkout(
        &self,
        worker_id: &str,
        branch: &str,
        expected_head: &str,
        capacity: WorkspaceCapacity,
    ) -> Result<TransactionWorkspace> {
        Box::pin(self.create_integration_checkout_inner(worker_id, branch, expected_head, capacity))
            .await
    }

    async fn create_integration_checkout_inner(
        &self,
        worker_id: &str,
        branch: &str,
        expected_head: &str,
        capacity: WorkspaceCapacity,
    ) -> Result<TransactionWorkspace> {
        validate_worker_id(worker_id)?;
        reject_option_like_ref("branch", branch)?;
        reject_option_like_ref("base", expected_head)?;
        if !matches!(expected_head.len(), 40 | 64)
            || !expected_head.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(SwarmError::WorktreeIo(
                "integration checkout requires an exact commit id".to_owned(),
            ));
        }

        self.validate_repo_authority()?;
        self.reject_executable_checkout_config().await?;
        self.assert_clean().await?;
        self.validate_swarm_root()?;
        let _admission = self.admission_lock.lock().await;
        let closure_bytes = self.transfer_closure_bytes(expected_head).await?;
        let checkout_bytes = self.checkout_logical_bytes(expected_head).await?;
        let initial_bytes = closure_bytes.checked_add(checkout_bytes).ok_or_else(|| {
            SwarmError::WorktreeIo("initial workspace size overflowed".to_owned())
        })?;
        let reserved_bytes = capacity.max_transaction_bytes;
        let required = reserved_bytes
            .checked_add(capacity.safety_margin_bytes)
            .ok_or_else(|| SwarmError::WorktreeIo("workspace capacity overflow".to_owned()))?;
        if initial_bytes > capacity.max_transaction_bytes {
            return Err(SwarmError::DispatchAdmission(format!(
                "workspace requires {initial_bytes} initial bytes ({closure_bytes} Git object bytes plus {checkout_bytes} logical checkout bytes), exceeding transaction budget {}",
                capacity.max_transaction_bytes
            )));
        }
        let transaction_root = self.swarm_root.join(worker_id);
        let (checkout, scratch, cleanup) = with_directory_lock(
            &self.swarm_root,
            &self.swarm_authority,
            || {
                let aggregate = self.reserved_workspace_bytes()?;
                if aggregate
                    .checked_add(reserved_bytes)
                    .is_none_or(|total| total > capacity.max_aggregate_bytes)
                {
                    return Err(SwarmError::DispatchAdmission(
                        "aggregate workspace budget exhausted".to_owned(),
                    ));
                }
                if aggregate
                    .checked_add(required)
                    .is_none_or(|total| total > capacity.available_bytes)
                {
                    return Err(SwarmError::DispatchAdmission(format!(
                        "workspace requires {required} bytes with {aggregate} already reserved, but authority proved only {} available bytes",
                        capacity.available_bytes,
                    )));
                }
                ensure_absent_destination(&transaction_root)?;
                std::fs::create_dir(&transaction_root).map_err(|error| {
                    io_at("creation of the transaction root", &transaction_root, error)
                })?;
                let registration = (|| {
                    make_guard_dir_private(&transaction_root).map_err(|error| {
                        io_at(
                            "private-mode set on the transaction root",
                            &transaction_root,
                            error,
                        )
                    })?;
                    // One representation for everything derived below: the root
                    // authority, the checkout/scratch joins, the lease file and
                    // TransactionCleanup.root. The checkout authority is opened on
                    // a path from this same derivation, which is what makes
                    // RetainedWorkspaceAuthority::new's parent-equality proof hold.
                    let transaction_root = normalized_root(&transaction_root)?;
                    let root_authority = DirectoryAuthority::open(&transaction_root)?;
                    let reservation_authority = Arc::new(
                        root_authority
                            .to_sandbox()
                            .create_child_file(
                                RESERVATION_FILE,
                                reserved_bytes.to_string().as_bytes(),
                            )
                            .map_err(|error| SwarmError::DispatchAdmission(error.to_string()))?,
                    );
                    let checkout = transaction_root.join("checkout");
                    let scratch = transaction_root.join("scratch");
                    std::fs::create_dir(&scratch)
                        .map_err(|error| io_at("creation of the scratch root", &scratch, error))?;
                    make_guard_dir_private(&scratch).map_err(|error| {
                        io_at("private-mode set on the scratch root", &scratch, error)
                    })?;
                    // The lease is taken on the LEASE_FILE that the transaction
                    // derivation opens-or-creates, never on the transaction-root
                    // DIRECTORY object: Windows byte-range locking is undefined
                    // on a directory handle. The separate create-and-drop of the
                    // lease file is gone because the lease handle now creates it.
                    // The loan is still accounted against this root's counter, so
                    // `remove_open_dir_all` keeps refusing while the lease is held.
                    let lease = ActiveLease::acquire(transaction_lease_handle(&root_authority)?)?;
                    let swarm_authority = DirectoryAuthority::open(&self.swarm_root)?;
                    let quarantine_authority = DirectoryAuthority::open(&self.control_root)?;
                    let root_identity = root_authority.identity_token();
                    let cleanup = Arc::new(TransactionCleanup {
                        owner: worker_id.to_owned(),
                        root: transaction_root.clone(),
                        root_authority: StdMutex::new(Some(root_authority)),
                        checkout_authority: std::sync::OnceLock::new(),
                        swarm_root: self.swarm_root.clone(),
                        swarm_authority,
                        quarantine_root: self.control_root.clone(),
                        quarantine_authority,
                        reservation_authority,
                        reserved_bytes,
                        active_reservations: Arc::clone(&self.active_reservations),
                        release_lock: StdMutex::new(()),
                        lease: StdMutex::new(Some(lease)),
                        released: AtomicBool::new(false),
                    });
                    self.active_reservations
                        .lock()
                        .map_err(|_| {
                            SwarmError::WorktreeIo(
                                "active reservation registry is poisoned".to_owned(),
                            )
                        })?
                        .insert(
                            worker_id.to_owned(),
                            ActiveReservation {
                                root_identity,
                                authority: Arc::clone(&cleanup.reservation_authority),
                                bytes: reserved_bytes,
                            },
                        );
                    Ok((checkout, scratch, cleanup))
                })();
                if registration.is_err() {
                    let _ = std::fs::remove_dir_all(&transaction_root);
                }
                registration
            },
        )?;
        // The clone boundary must still refer to the directory object retained
        // at construction, not a same-path replacement.
        self.validate_repo_authority()?;
        let source = self.repo_root.to_string_lossy().into_owned();
        let destination = checkout.to_string_lossy().into_owned();
        // Clone the target branch through the same object-isolating path
        // create_isolated_checkout uses (own object store, in-tree .git, no
        // alternates). Unlike the isolated path we check out the *existing*
        // branch (no fresh -b branch) so the result is the durable main working
        // tree the landing primitive re-projects onto.
        let clone_args = [
            "clone",
            "--no-local",
            "--no-hardlinks",
            "--depth=1",
            "--no-tags",
            "--single-branch",
            "--branch",
            branch,
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
                cleanup.release()?;
                return Err(SwarmError::WorktreeIo(format!(
                    "integration git clone failed: {error}"
                )));
            }
        };
        if !clone.status.success() {
            cleanup.release()?;
            return Err(SwarmError::WorktreeIo(format!(
                "integration git clone failed: {}",
                String::from_utf8_lossy(&clone.stderr).trim()
            )));
        }
        make_guard_dir_private(&checkout)
            .map_err(|error| io_at("private-mode set on the cloned checkout", &checkout, error))?;

        Box::pin(self.run_checkout_git(&checkout, &["remote", "remove", "origin"])).await?;

        // Prove the clone landed the exact requested tip on the target branch,
        // non-detached, with a clean working tree — the preconditions the
        // landing primitive enforces before it will bind the checkout.
        let actual = Box::pin(
            self.checkout_git_stdout(&checkout, &["rev-parse", "--verify", "HEAD^{commit}"]),
        )
        .await?;
        if actual != expected_head {
            return Err(SwarmError::WorktreeIo(format!(
                "integration checkout raced source branch: expected {expected_head}, got {actual}"
            )));
        }
        let head_ref =
            Box::pin(self.checkout_git_stdout(&checkout, &["symbolic-ref", "HEAD"])).await?;
        if head_ref != format!("refs/heads/{branch}") {
            return Err(SwarmError::WorktreeIo(format!(
                "integration checkout is not on target branch {branch}: HEAD is {head_ref}"
            )));
        }
        let status =
            Box::pin(self.checkout_git_stdout(&checkout, &["status", "--porcelain"])).await?;
        if !status.is_empty() {
            return Err(SwarmError::WorktreeIo(
                "integration checkout working tree is dirty".to_owned(),
            ));
        }

        let common =
            Box::pin(self.checkout_git_stdout(&checkout, &["rev-parse", "--git-common-dir"]))
                .await?;
        let common = PathBuf::from(common);
        let common = if common.is_absolute() {
            common
        } else {
            checkout.join(common)
        };
        let common = normalized_root(&common)?;
        let checkout_root = normalized_root(&checkout)?;
        if !common.starts_with(&checkout_root) {
            return Err(SwarmError::WorktreeIo(format!(
                "integration checkout Git authority escaped its root: {}",
                common.display()
            )));
        }
        // A main working tree keeps git_dir == common_git_dir; a linked worktree
        // does not. bind_integration_checkout refuses the linked case.
        let git_dir =
            Box::pin(self.checkout_git_stdout(&checkout, &["rev-parse", "--absolute-git-dir"]))
                .await?;
        let git_dir = normalized_root(Path::new(&git_dir))?;
        if git_dir != common {
            return Err(SwarmError::WorktreeIo(
                "integration checkout is a linked worktree; landing requires the main checkout"
                    .to_owned(),
            ));
        }
        if common != normalized_root(&checkout.join(".git"))? {
            return Err(SwarmError::WorktreeIo(
                "integration checkout does not own an in-tree .git directory".to_owned(),
            ));
        }
        let alternates = common.join("objects").join("info").join("alternates");
        if std::fs::symlink_metadata(&alternates).is_ok() {
            return Err(SwarmError::WorktreeIo(
                "integration checkout unexpectedly uses an alternate object store".to_owned(),
            ));
        }
        let remotes = Box::pin(self.checkout_git_stdout(&checkout, &["remote"])).await?;
        if !remotes.is_empty() {
            return Err(SwarmError::WorktreeIo(
                "integration checkout retained a remote".to_owned(),
            ));
        }
        let tags = Box::pin(self.checkout_git_stdout(&checkout, &["tag", "--list"])).await?;
        if !tags.is_empty() {
            return Err(SwarmError::WorktreeIo(
                "integration checkout retained tags".to_owned(),
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
        let checkout = normalized_root(&checkout)?;
        let scratch = normalized_root(&scratch)?;
        let transaction_root = normalized_root(&transaction_root)?;
        if !checkout.starts_with(&transaction_root)
            || !scratch.starts_with(&transaction_root)
            || checkout.starts_with(&scratch)
            || scratch.starts_with(&checkout)
        {
            return Err(SwarmError::WorktreeIo(
                "transaction checkout and scratch roots are not disjoint".to_owned(),
            ));
        }
        let authorities = Arc::new(TransactionWorkspaceAuthorities {
            // workstream C branch (i) — OBSERVATIONAL OPEN. Measured on
            // SEANDESKTOP, not inherited. A retained DELETE right makes
            // `SetCurrentDirectory` fail with a sharing violation, and
            // `git -C <checkout>` chdirs IN-PROCESS (MSYS `chdir()`), so a
            // DELETE-bearing checkout authority made every checkout-scoped git
            // invocation fail with "cannot change to ...: Permission denied".
            //
            // Determination 2: representation is INCIDENTAL, not causal — with
            // NO handle held, `git -C` succeeded with BOTH the verbatim
            // `\\?\C:\...` and the plain drive-letter form. Determination 3:
            // the checkout authority's complete consumer set is
            // `heartbeat::read_status_authorized` (relative child read),
            // `logical_tree_bytes` (read-only enumeration), `validate_path`
            // (identity), `CandidateSeal::mint` (manifest read), the loan-count
            // check in `TransactionCleanup::release`, and four `display_path()`
            // reads — NOTHING deletes, renames, creates a child or flushes
            // through it; the transaction root's destruction runs through the
            // ROOT authority's own recursion. Determination 4: the traverse
            // falsifier was tested directly rather than inherited from 20-72 —
            // through an observational handle, relative child reads, directory
            // enumeration, relative directory opens, identity checks and nested
            // reads ALL succeed.
            //
            // ANTI-SWAP IS UNCHANGED: the handle stays CONTINUOUSLY held, so
            // identity still survives a rename and no re-resolution window is
            // opened — byte-for-byte the property it had before. Widening this
            // back to `open` reintroduces the chdir refusal for every git
            // invocation scoped to this checkout, INCLUDING those made by CHILD
            // processes: the sharing conflict is system-wide, so a handle held
            // in the manager process denies a worker process's chdir just as
            // effectively.
            checkout: DirectoryAuthority::open_observational(&checkout)?,
            scratch: DirectoryAuthority::open(&scratch)?,
            reservation: Arc::clone(&cleanup.reservation_authority),
        });
        cleanup.bind_checkout_authority(authorities.checkout.clone());
        let workspace = TransactionWorkspace {
            owner: worker_id.to_owned(),
            root: transaction_root,
            checkout,
            scratch,
            base_commit: expected_head.to_owned(),
            head_commit,
            tree,
            reserved_bytes,
            authorities,
            cleanup,
        };
        let materialized_bytes = workspace.logical_used_bytes()?;
        if materialized_bytes > workspace.reserved_bytes {
            let reserved_bytes = workspace.reserved_bytes;
            workspace.cleanup.release()?;
            return Err(SwarmError::DispatchAdmission(format!(
                "materialized workspace uses {materialized_bytes} bytes, exceeding transaction budget {reserved_bytes}"
            )));
        }
        Ok(workspace)
    }
}

/// Per-transaction ceiling: the aggregate budget split fair-share across the
/// whole active roster, never above the absolute per-transaction cap.
fn per_transaction_ceiling(active_workers: u64) -> u64 {
    MAX_TRANSACTION_WORKSPACE_BYTES.min(MAX_AGGREGATE_WORKSPACE_BYTES / active_workers)
}

/// `(bytes charged for the newly admitted transactions, total free bytes the
/// host must prove)`.
///
/// Only the NEW transactions are charged a ceiling. Workspaces that already
/// exist arrive in `existing_reservation`, which `reserved_workspace_bytes()`
/// measured per root, so charging them a second ceiling here would count the
/// same disk twice.
fn admission_reservation(
    max_transaction_bytes: u64,
    admitted_transactions: u64,
    existing_reservation: u64,
) -> Option<(u64, u64)> {
    let admitted = max_transaction_bytes.checked_mul(admitted_transactions)?;
    let required = admitted
        .checked_add(existing_reservation)?
        .checked_add(WORKSPACE_SAFETY_MARGIN_BYTES)?;
    Some((admitted, required))
}

/// Host-independent pins for the admission arithmetic.
///
/// These deliberately do NOT touch the filesystem. The behavioural proof —
/// `spawner::production_durable_spawn_tests::concurrent_near_cap_admits_exactly_one_retained_workspace`
/// — can only discriminate on a host with less free space than the buggy
/// formula demanded, so on a large enough runner it would pass with the defect
/// present. These pin the arithmetic on every host.
#[cfg(test)]
mod admission_arithmetic_tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    /// The exact figure the spawner demanded before the parameter split:
    /// 256 workers * (64 GiB / 256) + 512 MiB.
    const DOUBLE_CHARGED_NEAR_CAP_BYTES: u64 = 69_256_347_648;

    #[test]
    fn one_admission_at_a_full_roster_charges_one_ceiling_not_the_roster() {
        let ceiling = per_transaction_ceiling(256);
        assert_eq!(
            ceiling,
            256 * MIB,
            "fair share of 64 GiB across 256 workers"
        );

        let (admitted, required) =
            admission_reservation(ceiling, 1, 0).expect("no overflow at one admission");
        assert_eq!(admitted, 256 * MIB);
        assert_eq!(required, 256 * MIB + 512 * MIB);

        // Regression pin. Charging the roster instead of the admission is what
        // made one small checkout demand 64.5 GiB of free space.
        assert_eq!(
            admission_reservation(ceiling, 256, 0)
                .expect("no overflow at roster charge")
                .1,
            DOUBLE_CHARGED_NEAR_CAP_BYTES
        );
        assert!(
            required < DOUBLE_CHARGED_NEAR_CAP_BYTES,
            "{required} must be far below the double-charged {DOUBLE_CHARGED_NEAR_CAP_BYTES}"
        );
    }

    #[test]
    fn a_fan_out_dispatch_still_charges_one_ceiling_per_new_worker() {
        // Four workers are four NEW transactions, so four ceilings is correct
        // and unchanged. This is a real, user-facing storage requirement: a
        // four-worker dispatch cannot start on a host with less than 32.5 GiB
        // free, whatever the repository's size, because each workspace is
        // allowed to grow to the 8 GiB per-transaction cap after admission.
        let ceiling = per_transaction_ceiling(4);
        assert_eq!(ceiling, 8 * GIB, "the absolute cap binds below 16 workers");

        let (admitted, required) =
            admission_reservation(ceiling, 4, 0).expect("no overflow at four workers");
        assert_eq!(admitted, 32 * GIB);
        assert_eq!(required, 32 * GIB + 512 * MIB);
    }

    #[test]
    fn already_reserved_bytes_are_charged_once_not_twice() {
        let ceiling = per_transaction_ceiling(256);
        let (admitted, required) = admission_reservation(ceiling, 1, 10 * GIB)
            .expect("no overflow with existing reservations");
        assert_eq!(admitted, 256 * MIB);
        assert_eq!(required, 10 * GIB + 256 * MIB + 512 * MIB);
    }

    #[test]
    fn an_overflowing_charge_refuses_rather_than_wrapping() {
        assert!(admission_reservation(u64::MAX, 2, 0).is_none());
        assert!(admission_reservation(u64::MAX, 1, 1).is_none());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod integration_checkout_tests {
    use super::*;
    use wcore_config::shell;

    async fn run_git(cwd: &Path, args: &[&str]) {
        let mut command = shell::shell_command_argv("git", args);
        command.current_dir(cwd);
        let status = command.status().await.expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    async fn git_stdout(cwd: &Path, args: &[&str]) -> String {
        let mut command = shell::shell_command_argv("git", args);
        command.current_dir(cwd);
        let out = command.output().await.expect("git output");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    async fn init_repo(path: &Path) -> String {
        run_git(path, &["init", "-q", "-b", "main"]).await;
        std::fs::write(path.join("README.md"), "integration fixture\n").unwrap();
        run_git(path, &["add", "."]).await;
        run_git(
            path,
            &[
                "-c",
                "user.email=swarm@test.invalid",
                "-c",
                "user.name=Swarm Test",
                "commit",
                "-qm",
                "fixture",
            ],
        )
        .await;
        git_stdout(path, &["rev-parse", "HEAD"]).await
    }

    #[tokio::test]
    async fn current_branch_reports_symbolic_head_and_fails_closed_on_detached() {
        let repo = tempfile::tempdir().expect("repo");
        let head = init_repo(repo.path()).await;
        let workspace_parent = tempfile::tempdir().expect("workspace parent");
        let workspace_root = workspace_parent.path().join("cb-swarm");
        let manager =
            WorktreeManager::new_with_workspace_root(repo.path(), &workspace_root).unwrap();

        // A normal branch checkout reports its symbolic HEAD.
        assert_eq!(manager.current_branch().await.unwrap(), "main");

        // A detached HEAD has no branch to land onto → fail closed.
        run_git(repo.path(), &["checkout", "-q", "--detach", &head]).await;
        assert!(
            manager.current_branch().await.is_err(),
            "a detached HEAD must fail closed"
        );
    }

    // A Wayland-owned integration checkout must satisfy every precondition the
    // 20-07 landing primitive (`bind_integration_checkout`) enforces. The proof
    // is direct: build the checkout, assert the observable bind requirements,
    // then hand the exact path to `bind_integration_checkout` and require that
    // it binds without error.
    #[tokio::test]
    async fn integration_checkout_satisfies_bind_requirements() {
        let repo = tempfile::tempdir().expect("repo");
        let head = init_repo(repo.path()).await;
        let workspace_parent = tempfile::tempdir().expect("workspace parent");
        let workspace_root = workspace_parent.path().join("integration-swarm");
        let manager =
            WorktreeManager::new_with_workspace_root(repo.path(), &workspace_root).unwrap();

        let capacity = WorkspaceCapacity {
            available_bytes: 1024 * 1024 * 1024,
            safety_margin_bytes: 0,
            max_transaction_bytes: 64 * 1024 * 1024,
            max_aggregate_bytes: 64 * 1024 * 1024,
        };
        let workspace = manager
            .create_integration_checkout("integrator", "main", &head, capacity)
            .await
            .expect("integration checkout");
        let checkout = workspace.checkout.clone();

        // Absolute path.
        assert!(checkout.is_absolute(), "checkout path must be absolute");
        // In-tree .git, and git_dir == common_git_dir (a main checkout, never a
        // linked worktree).
        let git_dir = std::fs::canonicalize(
            git_stdout(&checkout, &["rev-parse", "--absolute-git-dir"]).await,
        )
        .unwrap();
        let common = std::fs::canonicalize(
            git_stdout(
                &checkout,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await,
        )
        .unwrap();
        assert_eq!(
            git_dir, common,
            "must be a main checkout, not a linked worktree"
        );
        assert_eq!(
            common,
            std::fs::canonicalize(checkout.join(".git")).unwrap(),
            "must own an in-tree .git"
        );
        // No alternate object store.
        let alternates = common.join("objects").join("info").join("alternates");
        assert!(
            std::fs::symlink_metadata(&alternates).is_err(),
            "must not use an alternate object store"
        );
        // Clean working tree, on the expected branch, at the exact requested tip.
        assert!(
            git_stdout(&checkout, &["status", "--porcelain"])
                .await
                .is_empty(),
            "working tree must be clean"
        );
        assert_eq!(
            git_stdout(&checkout, &["symbolic-ref", "HEAD"]).await,
            "refs/heads/main",
            "must be a non-detached HEAD on the target branch"
        );
        assert_eq!(
            git_stdout(&checkout, &["rev-parse", "HEAD"]).await,
            head,
            "must be the exact requested tip"
        );

        // Strongest proof: the real 20-07 landing primitive accepts it.
        manager
            .bind_integration_checkout(&checkout)
            .await
            .expect("bind_integration_checkout must accept the Wayland-owned checkout");

        manager.release_transaction(&workspace).unwrap();
    }
}
