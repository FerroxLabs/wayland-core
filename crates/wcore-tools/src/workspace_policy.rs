//! `WorkspacePolicy` — the single source of truth for a session's
//! filesystem + network containment, installed at engine bootstrap.
//!
//! Two trust modes:
//!   * `Trusted` — local CLI / desktop sessions on the user's own machine.
//!     Roots the Bash OS-sandbox at the workspace (so builds see the
//!     workspace + toolchains — the pain fix), reuses global caches, keeps
//!     the network opt-in. The in-process file tools stay on `RealFs`
//!     (local file editing is not jailed).
//!   * `Contained` — remote `Workspace` posture. Tight write scope, caches
//!     redirected into the workspace, and the VFS layer wraps `RealFs` as
//!     `SandboxedFs ∘ SecretDenyFs`. (Bash is NOT in this posture yet — see
//!     the deferred OS-sandbox secret-read-deny work.)
//!
//! Network is ALWAYS seeded from `default_bash_network_policy()` so the
//! `WAYLAND_BASH_ALLOW_NETWORK` opt-in survives; it is never hardcoded.

use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use wcore_sandbox::manifest::NetworkPolicy;
use wcore_types::workspace_trust::DeveloperCapability;

const SECRET_SUFFIXES: &[&str] = &[
    "/.env",
    "/.git/config",
    "/.git-credentials",
    "/.npmrc",
    "/.pypirc",
    "/.netrc",
    "/.dockercfg",
    "/.aws/credentials",
    "/.kube/config",
    "/.git/hooks/",
    "/.docker/config.json",
    "/gradle.properties",
];

const SECRET_DIR_SEGMENTS: &[&str] = &["/.ssh/", "/.gnupg/", "/.aws/", "/.azure/", "/.gcloud/"];

const SECRET_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pfx", "tfstate"];

/// Extension-less secret basenames (SSH keys), matched on the final path
/// component.
const SECRET_BASENAMES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];

/// Cache vars redirected into `<root>/.wcache/<tool>` in `Contained` mode.
const CACHE_ENV_DIRS: &[(&str, &str)] = &[
    ("CARGO_HOME", "cargo"),
    ("npm_config_cache", "npm"),
    ("PIP_CACHE_DIR", "pip"),
];

/// User credential stores, $HOME-relative. NOTE the `.config/*` entries —
/// gcloud/gh/op live under ~/.config, NOT ~/.<name> (the v1 path bug).
/// Cross-checked against the existing SECRET_SUFFIXES/SEGMENTS so OS-deny
/// coverage is a superset of what the VFS `SecretDenyFs` already denies.
const CREDENTIAL_STORES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".npmrc",
    ".netrc",
    ".pgpass",
    ".pypirc",
    ".git-credentials",
    ".m2/settings.xml",
    ".gradle/gradle.properties",
    ".cargo/credentials.toml",
    ".terraform.d",
    ".bash_history",
    ".zsh_history",
    ".config/gcloud",
    ".config/gh",
    ".config/glab-cli",
    ".config/op",
    ".config/doctl",
];

/// Always-mounted system credential paths the backends grant unconditionally
/// (bwrap `--ro-bind /etc`; macOS allows `/Library`,`/System`). Emitted
/// regardless of `readable_roots()` because they ARE mounted. Kept short and
/// high-value — broad system reads remain a DAC + network-Deny residual.
#[cfg(target_os = "macos")]
const SYSTEM_CREDENTIAL_STORES: &[&str] = &["/Library/Keychains"];
#[cfg(target_os = "linux")]
const SYSTEM_CREDENTIAL_STORES: &[&str] = &["/etc/docker", "/etc/kubernetes"];
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const SYSTEM_CREDENTIAL_STORES: &[&str] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTrust {
    Trusted,
    Contained,
}

#[derive(Debug, Clone)]
pub struct WorkspacePolicy {
    root: PathBuf,
    trust: WorkspaceTrust,
    writable_extra: Vec<PathBuf>,
    readable_extra: Vec<PathBuf>,
    network: NetworkPolicy,
    cache_env: Vec<(String, String)>,
    /// Cached at construction time (once per session). Absolute, canonicalized
    /// paths that the OS-sandbox backend must deny for reads. See
    /// `secret_deny_paths()` / `compute_secret_deny()`.
    secret_deny: Vec<PathBuf>,
    /// Additional authority roots that must be unreadable to Bash even when a
    /// platform backend would otherwise expose them through a system mount.
    authority_read_deny: Vec<PathBuf>,
    /// Orchestrator authority roots that must not be covered by an external
    /// writable grant such as the host scratch directory. The child workspace
    /// root itself remains writable even when both happen to share an ancestor.
    authority_write_deny: Vec<PathBuf>,
    /// Strip Git environment overrides that could redirect a command from the
    /// contained checkout into orchestrator-owned repository administration.
    deny_git_authority_env: bool,
    delegated_scratch: Option<PathBuf>,
    /// #667: this policy relies on the OS sandbox actually enforcing
    /// `fs_read_deny` to keep secrets unreadable from `Bash` — so `Bash` must be
    /// REFUSED when the active backend cannot enforce read-deny (else it fails
    /// open). True for `Contained` and for any `Trusted` policy that opted into
    /// project-secret denial (`with_project_secret_deny`, i.e. Full/remote). A
    /// genuinely-local `Trusted` session leaves it false and keeps its shell.
    secret_read_deny_required: bool,
    developer_capabilities: Arc<RwLock<Vec<DeveloperCapability>>>,
    /// Read-only roots approved by the local desktop host for this process
    /// lifetime. This is interior-mutable so an already-running Bash tool sees
    /// the grant on its next call without replacing the session sandbox.
    session_read_grants: Arc<RwLock<Vec<PathBuf>>>,
}

#[derive(Debug, Error)]
pub enum WorkspaceCapabilityGrantError {
    #[error("session capability grants require a fingerprint-trusted local workspace")]
    RequiresTrustedLocal,
    #[error("capability path is not an executable regular file: {0}")]
    NotExecutable(PathBuf),
    #[error("capability executable resolves inside a credential store: {0}")]
    CredentialPath(PathBuf),
    #[error("capability path could not be resolved: {0}")]
    Resolve(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum DelegatedWorkspacePolicyError {
    #[error("delegated workspace path could not be resolved: {0}")]
    Resolve(#[from] std::io::Error),
    #[error("delegated checkout and scratch roots must be disjoint")]
    OverlappingRoots,
    #[error("delegated root overlaps protected authority: {0}")]
    AuthorityOverlap(PathBuf),
}

/// Map a retained-authority failure while materializing the private scratch
/// subdirectories into the delegated-workspace error surface.
fn delegated_scratch_error(error: wcore_sandbox::SandboxError) -> DelegatedWorkspacePolicyError {
    DelegatedWorkspacePolicyError::Resolve(std::io::Error::other(error.to_string()))
}

impl WorkspacePolicy {
    /// Local/desktop session on the user's own machine. Roots the sandbox
    /// at `workspace`, allows the workspace + user toolchains/caches so
    /// builds and installs work, reuses global caches (no redirect), and
    /// honors the network opt-in. Does NOT jail the in-process file tools.
    pub fn trusted_local(workspace: impl Into<PathBuf>) -> Self {
        let root = canon(workspace.into());
        let mut writable_extra = scratch_dirs();
        if let Some(home) = dirs::home_dir() {
            for sub in [".cache", ".cargo/registry", ".cargo/git", ".npm/_cacache"] {
                let path = home.join(sub);
                if path.exists() {
                    writable_extra.push(canon(path));
                }
            }
        }
        let developer_capabilities = detect_developer_capabilities();
        let mut readable_extra = developer_capabilities
            .iter()
            .flat_map(|capability| capability.read_only_roots.iter())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        readable_extra.extend(trusted_config_and_certificate_reads());
        readable_extra.sort();
        readable_extra.dedup();

        // Compute readable_canon from the same locals readable_roots() uses.
        let readable_canon = readable_canon_roots(&root, &writable_extra, &readable_extra);
        let secret_deny = compute_secret_deny(WorkspaceTrust::Trusted, &root, &readable_canon);

        Self {
            root,
            trust: WorkspaceTrust::Trusted,
            writable_extra,
            readable_extra,
            // #657: the bare constructor is fail-safe — network is seeded from
            // `default_bash_network_policy()` (Deny unless `WAYLAND_BASH_ALLOW_NETWORK`).
            // Network egress is granted only for a GENUINELY-LOCAL session, and
            // that grant is applied at bootstrap via `with_network(Inherit)` gated
            // on `channel_tool_posture.is_none()` (see `local_bash_network`). A
            // channel-attached session — including `Full` posture — is a remote
            // sender and stays on this Deny default: it must not get a networked
            // shell by default (Overwatch ruling on #657, Sean-confirmed).
            network: crate::bash::default_bash_network_policy(),
            cache_env: Vec::new(),
            secret_deny,
            authority_read_deny: Vec::new(),
            authority_write_deny: Vec::new(),
            deny_git_authority_env: false,
            delegated_scratch: None,
            // Genuinely-local Trusted default: no project-secret denial, so the
            // Bash read-deny-enforcement gate does not apply. `with_project_secret_deny`
            // flips this to true for a Full/remote session (#667).
            secret_read_deny_required: false,
            developer_capabilities: Arc::new(RwLock::new(developer_capabilities)),
            session_read_grants: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Remote `Workspace` posture. Tight write scope, caches redirected into
    /// the workspace, network opt-in preserved. The caller layers
    /// `SandboxedFs ∘ SecretDenyFs` on the VFS using `is_secret_path`.
    pub fn contained(root: impl Into<PathBuf>) -> Self {
        let root = canon(root.into());
        let cache_root = root.join(".wcache");
        let cache_env = CACHE_ENV_DIRS
            .iter()
            .map(|(var, sub)| {
                (
                    (*var).to_string(),
                    cache_root.join(sub).to_string_lossy().into_owned(),
                )
            })
            .collect();
        let readable_extra = minimal_toolchain_read_dirs();
        // Hoist writable_extra so we can borrow it for readable_canon.
        let writable_extra = scratch_dirs();

        // Compute readable_canon from the same locals readable_roots() uses.
        let readable_canon = readable_canon_roots(&root, &writable_extra, &readable_extra);
        let secret_deny = compute_secret_deny(WorkspaceTrust::Contained, &root, &readable_canon);

        Self {
            root,
            trust: WorkspaceTrust::Contained,
            writable_extra,
            readable_extra,
            // #657: a Contained (untrusted / remote `Workspace`) posture runs
            // potentially attacker-influenced content, so egress stays DENIED to
            // keep the exfil boundary tight. `WAYLAND_BASH_ALLOW_NETWORK=1`
            // remains the explicit operator escape hatch (via
            // `default_bash_network_policy`).
            network: crate::bash::default_bash_network_policy(),
            cache_env,
            secret_deny,
            authority_read_deny: Vec::new(),
            authority_write_deny: Vec::new(),
            deny_git_authority_env: false,
            delegated_scratch: None,
            // Contained denies project secrets → Bash must be refused when the
            // backend can't enforce read-deny (else `cat .env` fails open).
            secret_read_deny_required: true,
            developer_capabilities: Arc::new(RwLock::new(Vec::new())),
            session_read_grants: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Build the write policy for one owner-issued delegated mutation.
    /// Global scratch and cache paths are deliberately excluded.
    pub fn delegated_mutation(
        checkout: impl AsRef<Path>,
        scratch: impl AsRef<Path>,
        protected_authority: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, DelegatedWorkspacePolicyError> {
        let checkout = std::fs::canonicalize(checkout)?;
        let scratch = std::fs::canonicalize(scratch)?;
        if checkout.starts_with(&scratch) || scratch.starts_with(&checkout) {
            return Err(DelegatedWorkspacePolicyError::OverlappingRoots);
        }
        let mut protected = protected_authority
            .into_iter()
            .map(std::fs::canonicalize)
            .collect::<std::io::Result<Vec<_>>>()?;
        protected.sort();
        protected.dedup();
        for authority in &protected {
            if checkout.starts_with(authority)
                || authority.starts_with(&checkout)
                || scratch.starts_with(authority)
                || authority.starts_with(&scratch)
            {
                return Err(DelegatedWorkspacePolicyError::AuthorityOverlap(
                    authority.clone(),
                ));
            }
        }

        let readable_extra = minimal_toolchain_read_dirs();
        let writable_extra = vec![scratch.clone()];
        let readable_canon = readable_canon_roots(&checkout, &writable_extra, &readable_extra);
        let secret_deny =
            compute_secret_deny(WorkspaceTrust::Contained, &checkout, &readable_canon);
        let mut cache_env = CACHE_ENV_DIRS
            .iter()
            .map(|(var, sub)| {
                (
                    (*var).to_string(),
                    scratch
                        .join("cache")
                        .join(sub)
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect::<Vec<_>>();
        cache_env.extend(["TMPDIR", "TMP", "TEMP"].into_iter().map(|var| {
            (
                var.to_owned(),
                scratch.join("tmp").to_string_lossy().into_owned(),
            )
        }));

        // The delegated child's TMPDIR/TMP/TEMP and tool caches resolve UNDER
        // the private scratch root; those subdirectories must exist and be
        // usable. Materialize them through the retained scratch authority
        // (owner-relative openat/mkdirat, never a raw absolute-path reopen) so
        // legitimate mutation into the private scratch cannot fail with ENOENT.
        // Only paths already inside the writable scratch grant are created; the
        // parent/global-temp/symlink/secret denials are unaffected.
        let scratch_authority =
            wcore_sandbox::DirectoryAuthority::open(&scratch).map_err(delegated_scratch_error)?;
        scratch_authority
            .open_or_create_child_directory("tmp")
            .map_err(delegated_scratch_error)?;
        let cache_root = scratch_authority
            .open_or_create_child_directory("cache")
            .map_err(delegated_scratch_error)?;
        for (_, sub) in CACHE_ENV_DIRS {
            cache_root
                .open_or_create_child_directory(sub)
                .map_err(delegated_scratch_error)?;
        }

        Ok(Self {
            root: checkout,
            trust: WorkspaceTrust::Contained,
            writable_extra,
            readable_extra,
            network: crate::bash::default_bash_network_policy(),
            cache_env,
            secret_deny,
            authority_read_deny: protected.clone(),
            authority_write_deny: protected,
            deny_git_authority_env: true,
            delegated_scratch: Some(scratch),
            secret_read_deny_required: true,
            developer_capabilities: Arc::new(RwLock::new(Vec::new())),
            session_read_grants: Arc::new(RwLock::new(Vec::new())),
        })
    }

    pub fn trust(&self) -> WorkspaceTrust {
        self.trust
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn writable_roots(&self) -> Vec<PathBuf> {
        let mut v = Vec::with_capacity(1 + self.writable_extra.len());
        v.push(self.root.clone());
        v.extend(
            self.writable_extra
                .iter()
                .filter(|candidate| {
                    !self.authority_write_deny.iter().any(|denied| {
                        denied.starts_with(candidate.as_path())
                            || candidate.starts_with(denied.as_path())
                    })
                })
                .cloned(),
        );
        v
    }
    pub fn readable_roots(&self) -> Vec<PathBuf> {
        let mut v = self.writable_roots();
        v.extend(self.readable_extra.iter().cloned());
        v.extend(self.session_read_grants.read().iter().cloned());
        v.sort();
        v.dedup();
        v
    }
    pub fn network(&self) -> NetworkPolicy {
        self.network.clone()
    }

    /// Override the network posture. Used at bootstrap to grant `Inherit` to a
    /// genuinely-local session (see [`local_bash_network`]); the bare
    /// constructors stay on the fail-safe Deny default.
    pub fn with_network(mut self, network: NetworkPolicy) -> Self {
        self.network = network;
        self
    }
    pub fn cache_env(&self) -> &[(String, String)] {
        &self.cache_env
    }

    /// Absolute, canonicalized paths that the OS-sandbox backend must deny
    /// for reads. Computed once at construction (cached). Empty when no deny
    /// applies (no `$HOME`, no workspace root, etc.) — empty = today's
    /// behavior for callers that don't set `manifest.fs_read_deny`.
    pub fn secret_deny_paths(&self) -> &[PathBuf] {
        &self.secret_deny
    }

    /// True if `path` is a secret that must stay denied even inside a
    /// writable root. Lexical; the VFS adapter calls this with the
    /// already-canonicalized path (see `SecretDenyFs`), so symlinks that
    /// resolve to a secret inside the root are caught.
    pub fn is_secret_path(&self, path: &Path) -> bool {
        is_secret_path_static(path)
    }

    /// #667 (Overwatch ruling, Sean-confirmed): true when `path` is a
    /// PROJECT-committed secret — a secret-named file UNDER this policy's
    /// workspace root (`.env`, `service-account*.json`, `*.pem`, …). Used as
    /// the `SecretDenyFs` read-path predicate so a `Full`-posture channel /
    /// remote sender cannot `Read`/`Write`/`Edit` the project's own secrets.
    ///
    /// Deliberately WORKSPACE-SCOPED (not bare `is_secret_path`): a host
    /// secret OUTSIDE the workspace root (`~/.aws/credentials`, `~/.ssh/id_rsa`)
    /// stays readable, because `Full` posture is the deliberate
    /// trusted-remote-operator escape hatch ("identical to a local CLI
    /// session") and the ruling scopes the NEW denial to project secrets only.
    /// Lexical name-match (not the construction-time walk) so a `.env` written
    /// AFTER the session starts is still caught — no TOCTOU gap.
    ///
    /// CANONICALIZE-FIRST: both the name match and the under-root check run on
    /// the symlink-resolved, real-cased path. In the Full deployment there is no
    /// `SandboxedFs` wrapper to pre-canonicalize (unlike the Workspace jail), so
    /// matching the raw path would let a benign-named symlink (`notes.txt` →
    /// `.env`) or a case-variant (`.ENV` on a case-insensitive FS) slip a
    /// project secret through. Resolving first closes both (#667 F3/F4). This is
    /// exactly the canonical path the Workspace jail already feeds in, so the
    /// Contained deployment is unchanged.
    pub fn is_project_secret(&self, path: &Path) -> bool {
        let canon = canon_for_scope(path);
        is_secret_path_static(&canon) && canon.starts_with(&self.root)
    }

    /// #667: opt a `Trusted` policy into the same PROJECT-committed-secret
    /// denial (`secret_deny_paths()`) that `Contained` applies, so a
    /// `Full`-posture channel / remote session's `Bash` OS-sandbox refuses to
    /// read the workspace's own secrets. A GENUINELY-LOCAL keyboard session
    /// (no channel posture) does NOT call this — the operator may read their
    /// own `.env`. Complements the `SecretDenyFs` read-path guard installed for
    /// the same sessions at bootstrap. Idempotent (sort + dedup).
    pub fn with_project_secret_deny(mut self) -> Self {
        let readable_canon =
            readable_canon_roots(&self.root, &self.writable_extra, &self.readable_extra);
        self.secret_deny
            .extend(project_committed_secrets(&self.root, &readable_canon));
        self.secret_deny.sort();
        self.secret_deny.dedup();
        // #667 F2: this Trusted policy now denies project secrets, so its `Bash`
        // must also be refused when the backend can't enforce read-deny.
        self.secret_read_deny_required = true;
        self
    }

    /// Deny explicit orchestrator authority roots to shell commands.
    pub fn with_authority_read_deny(mut self, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        self.authority_read_deny.extend(roots);
        self.authority_read_deny.sort();
        self.authority_read_deny.dedup();
        self.secret_read_deny_required = true;
        self
    }

    /// Remove every external writable grant that contains an orchestrator
    /// authority root. This is the write-side complement to
    /// [`Self::with_authority_read_deny`].
    pub fn with_authority_write_deny(mut self, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        self.authority_write_deny.extend(roots);
        self.authority_write_deny.sort();
        self.authority_write_deny.dedup();
        self
    }

    /// Prevent inherited/session-allowed Git variables from redirecting Bash
    /// outside this policy's workspace.
    pub fn with_git_authority_env_deny(mut self) -> Self {
        self.deny_git_authority_env = true;
        self
    }

    #[must_use]
    pub fn denies_git_authority_env(&self) -> bool {
        self.deny_git_authority_env
    }

    #[must_use]
    pub fn delegated_scratch(&self) -> Option<&Path> {
        self.delegated_scratch.as_deref()
    }

    /// Revalidate transaction roots immediately before process spawn.
    pub fn delegated_roots_are_current(&self) -> bool {
        let Some(scratch) = self.delegated_scratch.as_ref() else {
            return true;
        };
        let Ok(root_now) = std::fs::canonicalize(&self.root) else {
            return false;
        };
        let Ok(scratch_now) = std::fs::canonicalize(scratch) else {
            return false;
        };
        root_now == self.root
            && scratch_now == *scratch
            && !root_now.starts_with(&scratch_now)
            && !scratch_now.starts_with(&root_now)
            && self.writable_roots() == vec![root_now, scratch_now]
    }

    /// #234: the OS-sandbox read-deny list AS OF NOW, recomputed per Bash exec.
    ///
    /// Identical to [`secret_deny_paths`](Self::secret_deny_paths) EXCEPT it
    /// re-walks the workspace for project-committed secrets, so a secret CREATED
    /// AFTER bootstrap (a pulled `*.pem`, a generated `terraform.tfstate`) is
    /// denied on the very next Bash command. This closes the TOCTOU gap between
    /// the frozen construction-time list — which `bash.rs` fed to the OS sandbox
    /// — and the dynamic [`is_project_secret`](Self::is_project_secret) guard the
    /// in-process file tools (`SecretDenyFs`) already enforce per-access. Before
    /// this, `Bash cat terraform.tfstate` could read a secret that `Read` refused.
    ///
    /// Scope: this closes the CROSS-command window (a secret created by an earlier
    /// command, read by a later one). The INTRA-command window is inherent to a
    /// static pre-exec OS-sandbox deny list and is NOT closed — a single compound
    /// command that both creates and reads a secret (`terraform apply && cat
    /// terraform.tfstate`) generates it AFTER this walk, so it is absent for that
    /// exec. The file tools' per-access guard covers that case; `Bash`-as-subprocess
    /// structurally cannot. Exfil is blunted by the default `network = Deny`.
    ///
    /// Gated on [`secret_read_deny_required`](Self::secret_read_deny_required):
    /// only postures that ALREADY deny project secrets (Contained, or Full/remote
    /// via [`with_project_secret_deny`](Self::with_project_secret_deny)) get the
    /// fresh walk. A genuinely-local keyboard session (Trusted, flag unset) is
    /// returned UNCHANGED — the operator may still read their own `.env` (Sean's
    /// #667 ruling). Reuses the SAME `project_committed_secrets` walk the frozen
    /// list is built from, so the two cannot drift and its anti-bypass properties
    /// (a `.gitignore`d `.env` is still denied, a symlink-to-secret is masked,
    /// only under-mounted paths are emitted) are inherited verbatim.
    ///
    /// Also denies the git CONTENT stores ([`git_content_stores`]) so a committed
    /// secret cannot be reconstructed from `.git/objects` via `Bash("git show
    /// HEAD:.env")` and friends — the sibling of the typed-GitTool drop (MF1).
    pub fn secret_deny_paths_dynamic(&self) -> Vec<PathBuf> {
        // Recompute the base deny set against the CURRENT readable roots. A
        // desktop capability grant can add a read-only runtime mount after
        // bootstrap; using the construction-time cache here would expose any
        // credential store newly brought under that mount.
        let mut readable_canon = self
            .readable_roots()
            .into_iter()
            .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
            .collect::<Vec<_>>();
        readable_canon.sort();
        readable_canon.dedup();
        // Add project secrets exactly once below for every posture that
        // requires them. Passing Trusted here avoids a duplicate workspace
        // walk for Contained policies.
        let base_trust = if self.secret_read_deny_required {
            WorkspaceTrust::Trusted
        } else {
            self.trust
        };
        let mut out = compute_secret_deny(base_trust, &self.root, &readable_canon);
        if self.secret_read_deny_required {
            out.extend(project_committed_secrets(&self.root, &readable_canon));
            out.extend(git_content_stores(&self.root));
        }
        out.extend(self.authority_read_deny.iter().cloned());
        out.sort();
        out.dedup();
        out
    }

    /// #667 (F2): true when `Bash` must be REFUSED on a backend that cannot
    /// enforce `fs_read_deny` at the OS layer — because this policy relies on
    /// that enforcement to keep secrets unreadable from the shell. Replaces the
    /// old `trust() == Contained` proxy in `bash.rs`, which #667 invalidated by
    /// minting a `Trusted` policy (Full/remote) that also requires enforcement.
    pub fn secret_read_deny_required(&self) -> bool {
        self.secret_read_deny_required
    }

    pub fn developer_capabilities(&self) -> Vec<DeveloperCapability> {
        self.developer_capabilities.read().clone()
    }

    /// Add a read-only developer runtime capability for this session.
    ///
    /// The caller supplies an executable selected by the local desktop UI.
    /// Core canonicalizes it, derives the minimum known runtime roots, and
    /// never widens writable roots or disables the sandbox. Contained,
    /// Managed and remote sessions use `WorkspaceTrust::Contained`, so they
    /// fail closed here even if a wire peer guesses this command.
    pub fn grant_session_capability(
        &self,
        executable: impl AsRef<Path>,
    ) -> Result<DeveloperCapability, WorkspaceCapabilityGrantError> {
        if self.trust != WorkspaceTrust::Trusted {
            return Err(WorkspaceCapabilityGrantError::RequiresTrustedLocal);
        }
        let executable = std::fs::canonicalize(executable)?;
        let metadata = std::fs::metadata(&executable)?;
        if !metadata.is_file() {
            return Err(WorkspaceCapabilityGrantError::NotExecutable(executable));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(WorkspaceCapabilityGrantError::NotExecutable(executable));
            }
        }
        if path_is_in_credential_store(&executable) {
            return Err(WorkspaceCapabilityGrantError::CredentialPath(executable));
        }
        let mut roots = capability_roots(&executable);
        roots.sort();
        roots.dedup();
        {
            let mut grants = self.session_read_grants.write();
            grants.extend(roots.iter().cloned());
            grants.sort();
            grants.dedup();
        }
        let capability = DeveloperCapability {
            name: executable
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("custom_tool")
                .to_string(),
            executable: executable.to_string_lossy().into_owned(),
            read_only_roots: roots
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        };
        let mut capabilities = self.developer_capabilities.write();
        if !capabilities
            .iter()
            .any(|existing| existing.executable == capability.executable)
        {
            capabilities.push(capability.clone());
        }
        Ok(capability)
    }
}

fn path_is_in_credential_store(path: &Path) -> bool {
    if let Some(home) = dirs::home_dir() {
        for relative in CREDENTIAL_STORES {
            let store = home.join(relative);
            let store = std::fs::canonicalize(&store).unwrap_or(store);
            if path.starts_with(store) {
                return true;
            }
        }
    }
    SYSTEM_CREDENTIAL_STORES
        .iter()
        .map(Path::new)
        .any(|store| path.starts_with(store))
}

/// Free-function body of `is_secret_path` (uses no `self` fields). Extracted
/// so `compute_secret_deny` can call it without a `WorkspacePolicy` instance.
fn is_secret_path_static(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");

    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && SECRET_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
    {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if SECRET_BASENAMES.contains(&name) {
            return true;
        }
        // service-account*.json, bare key.json, and separator-bounded *-key.json / *_key.json.
        // Does NOT match monkey.json, turnkey.json, hotkey.json (no false positives).
        if name.ends_with(".json")
            && (name.starts_with("service-account")
                || name == "key.json"
                || name.ends_with("-key.json")
                || name.ends_with("_key.json"))
        {
            return true;
        }
        // terraform.tfstate and terraform.tfstate.backup (compound extension)
        if name.contains(".tfstate") {
            return true;
        }
    }
    if SECRET_DIR_SEGMENTS.iter().any(|seg| s.contains(seg)) {
        return true;
    }
    SECRET_SUFFIXES.iter().any(|frag| {
        if frag.ends_with('/') {
            s.contains(frag)
        } else if let Some(idx) = s.rfind(frag) {
            let after = &s[idx + frag.len()..];
            after.is_empty() || after.starts_with('.') || after.starts_with('/')
        } else {
            false
        }
    })
}

/// Compute the set of paths that must be denied for reading in the OS sandbox.
///
/// `readable_canon` must be already-canonicalized readable roots (from the
/// same locals that `readable_roots()` uses). BOTH sides of the under-mounted
/// check are canonicalized to avoid macOS `/var` → `/private/var` mismatches
/// (a fail-open bug if skipped).
///
/// Emits a path when it is under a readable/mounted root OR an always-on
/// system mount. Sorted + deduped.
fn compute_secret_deny(
    trust: WorkspaceTrust,
    root: &Path,
    readable_canon: &[PathBuf],
) -> Vec<PathBuf> {
    // Always-on system credential mounts (unconditionally granted by backends).
    let system_roots: Vec<PathBuf> = SYSTEM_CREDENTIAL_STORES.iter().map(PathBuf::from).collect();

    // A path is mountable if it is under a readable root OR an always-on
    // system mount. BOTH sides must already be canonicalized for this to be
    // correct on macOS (where /var -> /private/var).
    let under_mounted = |p: &Path| {
        readable_canon.iter().any(|r| p.starts_with(r))
            || system_roots.iter().any(|r| p.starts_with(r))
    };

    let mut out: Vec<PathBuf> = Vec::new();

    // User credential stores (both Trusted and Contained modes).
    if let Some(home) = dirs::home_dir() {
        for rel in CREDENTIAL_STORES {
            // Canonicalize the candidate path so both sides match.
            if let Ok(c) = std::fs::canonicalize(home.join(rel))
                && under_mounted(&c)
            {
                out.push(c);
            }
        }
    }

    // Wayland's OWN per-profile credential + OAuth stores (both modes). The
    // active profile home is often inside $HOME, so it is mountable into a
    // Trusted sandbox — and an LLM-driven bash command must not be able to
    // `cat` the profile's secrets. Covers the plaintext-0600 fallback
    // (credentials.toml), the encrypted vault blob + KDF params
    // (credentials.enc / credentials.kdf.json — the passphrase is never
    // forwarded, but deny the blob so it cannot be exfiltrated for offline
    // attack), and the OAuth token dir. Resolves via the same WAYLAND_HOME-aware
    // helpers the credential store itself uses, so non-default profile homes are
    // covered too. `under_mounted` keeps homes outside readable roots out of the
    // list (they are not reachable from the sandbox anyway).
    let cred_dir = wcore_config::config::wayland_config_dir();
    for name in [
        "credentials.toml",
        "credentials.enc",
        "credentials.kdf.json",
    ] {
        if let Ok(c) = std::fs::canonicalize(cred_dir.join(name))
            && under_mounted(&c)
        {
            out.push(c);
        }
    }
    if let Ok(c) = std::fs::canonicalize(wcore_config::config::profile_home().join("oauth"))
        && under_mounted(&c)
    {
        out.push(c);
    }

    // Always-mounted system credential stores (both modes). Emit if they
    // exist on disk; canonicalize so the path is exact.
    for s in &system_roots {
        if let Ok(c) = std::fs::canonicalize(s) {
            out.push(c);
        }
    }

    // Contained mode also denies the workspace's own committed secrets.
    // #667: `with_project_secret_deny` reuses `project_committed_secrets` to
    // apply the SAME denial to a `Full`-posture channel/remote `Trusted` policy.
    if trust == WorkspaceTrust::Contained {
        out.extend(project_committed_secrets(root, readable_canon));
    }

    out.sort();
    out.dedup();
    out
}

/// Absolute, canonicalized paths of the workspace's OWN committed secrets
/// (`.env`, `service-account*.json`, `*.pem`, …) that are reachable from a
/// sandbox mounted at `root`. Walks `root` ignoring `.gitignore` (a
/// gitignored `.env` must still be denied) and emits a path only when it is
/// under a readable/mounted root. Shared by `compute_secret_deny` (Contained)
/// and `WorkspacePolicy::with_project_secret_deny` (#667, Full/remote Trusted)
/// so the two paths cannot drift.
fn project_committed_secrets(root: &Path, readable_canon: &[PathBuf]) -> Vec<PathBuf> {
    let system_roots: Vec<PathBuf> = SYSTEM_CREDENTIAL_STORES.iter().map(PathBuf::from).collect();
    let under_mounted = |p: &Path| {
        readable_canon.iter().any(|r| p.starts_with(r))
            || system_roots.iter().any(|r| p.starts_with(r))
    };

    let mut out: Vec<PathBuf> = Vec::new();
    // NO directory prune: the file tools' `is_project_secret` predicate covers a
    // secret ANYWHERE under root, so this list must too — pruning `node_modules`/
    // `target`/`.wcache` would deny a committed secret to Read/Edit/Grep while
    // leaving it READABLE via `Bash cat node_modules/vendor/x.pem` (the two layers
    // must not diverge). The per-exec #234 DoS is killed instead by a LEXICAL
    // prefilter: we canonicalize (an expensive symlink-resolving syscall) ONLY for
    // secret-NAMED files and for symlinks — not for every entry. Visiting (readdir)
    // a large `node_modules` is cheap; canonicalizing every file in it was the cost.
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(false) // a .gitignore'd .env must still be denied
        .hidden(false)
        .follow_links(false)
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        let is_symlink = entry.path_is_symlink();
        if !is_symlink {
            // Regular file: cheap lexical check on the raw name FIRST; only a
            // secret-named file is worth the canonicalize syscall.
            if !entry.file_type().is_some_and(|t| t.is_file()) || !is_secret_path_static(path) {
                continue;
            }
            if let Ok(canon) = std::fs::canonicalize(path)
                && under_mounted(&canon)
            {
                out.push(canon);
            }
            continue;
        }
        // Symlink (rare): resolve the target and deny the link's own canonical
        // path if the TARGET is a secret, masking a benign-named link to a secret
        // (`notes.txt` → `.env`). Must canonicalize regardless of the link's name.
        // External-target residual (target not under a mounted root) is documented
        // in the plan — backstopped by network-Deny.
        if let Ok(canon) = std::fs::canonicalize(path)
            && is_secret_path_static(&canon)
            && under_mounted(&canon)
        {
            out.push(canon);
        }
    }
    out
}

/// Git CONTENT stores under `root` that must be OS-sandbox-denied for reads in a
/// secret-deny posture. A committed secret's bytes live in the object store, NOT
/// as a working-tree path, so `Bash("git show HEAD:.env")` / `git cat-file` /
/// `git log -p` / `git blame` reconstruct the committed secret from there,
/// sailing past the working-tree `.env` deny. The typed `GitTool` is already
/// dropped in these postures (MF1); denying the object store closes the sibling
/// Bash+git door ROBUSTLY — one mechanism kills every content-emitting git verb
/// and every shell-syntax variant, versus enumerating git's sprawling read
/// surface. `.git/refs`/`HEAD` stay readable, so `git rev-parse` (a SHA, no
/// content) still works. Covers the main store, submodule stores (`.git/modules`)
/// and LFS (`.git/lfs`). Empirically verified on the box (bwrap `--tmpfs` shadows
/// the dir → `git show`/`cat-file`/`log -p` all fail).
fn git_content_stores(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for rel in [".git/objects", ".git/modules", ".git/lfs"] {
        let p = root.join(rel);
        if p.exists() {
            out.push(std::fs::canonicalize(&p).unwrap_or(p));
        }
    }
    out
}

/// Canonicalized readable roots (workspace + writable + readable extras), the
/// same set `readable_roots()` exposes. Both sides of the under-mounted check
/// must be canonicalized so macOS `/var` → `/private/var` matches.
fn readable_canon_roots(
    root: &Path,
    writable_extra: &[PathBuf],
    readable_extra: &[PathBuf],
) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::iter::once(root.to_path_buf())
        .chain(writable_extra.iter().cloned())
        .chain(readable_extra.iter().cloned())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Best-effort canonicalization for the under-root scope check. Falls back to
/// canonicalizing the parent + re-attaching the final component when `path`
/// itself does not exist (e.g. a `Write` to a not-yet-created `.env`), so the
/// `/var` → `/private/var` normalization still lands and the prefix match
/// against the canonical root holds.
fn canon_for_scope(path: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|p| p.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

fn canon(p: PathBuf) -> PathBuf {
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn scratch_dirs() -> Vec<PathBuf> {
    let tmp = std::env::temp_dir();
    vec![canon(tmp)]
}

/// #657 (Overwatch ruling, Sean-confirmed): the Bash network posture for a
/// `Trusted` workspace is `Inherit` (egress ON — npm/pip/cargo/brew installs,
/// curl, git fetch just work) ONLY for a GENUINELY-LOCAL session: one with no
/// channel posture attached (local CLI / TUI / json-stream / ACP / desktop).
///
/// A channel-attached session — INCLUDING `Full` posture — is a remote sender.
/// It stays on the pre-#657 lockdown: `default_bash_network_policy()` (Deny
/// unless the operator sets `WAYLAND_BASH_ALLOW_NETWORK`). A remote-triggered
/// context does not get a networked shell by default; if a real
/// remote-networked-shell use case appears, it becomes a deliberate per-channel
/// opt-in, not the default.
pub fn local_bash_network(has_channel_posture: bool) -> NetworkPolicy {
    if has_channel_posture {
        crate::bash::default_bash_network_policy()
    } else {
        NetworkPolicy::Inherit
    }
}

mod discovery;
use discovery::{
    capability_roots, detect_developer_capabilities, minimal_toolchain_read_dirs,
    trusted_config_and_certificate_reads,
};

#[cfg(test)]
mod tests;
