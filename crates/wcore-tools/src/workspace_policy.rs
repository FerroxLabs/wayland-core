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
//! Network is ALWAYS seeded from `default_bash_network_policy()` — a fail-safe
//! Deny — and widened only by an explicit `with_network` at the trusted
//! bootstrap seam (`local_bash_network` for a genuinely-local session,
//! `operator_bash_network` for a sandboxed one). It is never hardcoded here.

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

/// System credential paths denied to the child regardless of
/// `readable_roots()`, because a backend may mount them without the policy
/// having asked for them. Kept short and high-value — broad system reads remain
/// a DAC + network-Deny residual.
///
/// **macOS: still literally always-mounted.** The seatbelt profile allows
/// `/Library` and `/System`, so `/Library/Keychains` is inside the sandbox and
/// this deny is the only thing keeping it out.
///
/// **Linux: no longer always-mounted, and this entry is now defence in depth.**
/// It used to be load-bearing against the blanket `--ro-bind /etc /etc` the
/// bwrap backend emitted; `SYSTEM_RO_ETC` (SEC-05/07/10) replaced that with a
/// curated list that contains neither `/etc/docker` nor `/etc/kubernetes`, so
/// those paths are absent from the namespace entirely and the deny mount is
/// classified `Absent` and dropped. Keeping the entries costs nothing and keeps
/// the denial correct if `SYSTEM_RO_ETC` ever grows or a caller grants an
/// ancestor through `fs_read_allow`.
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
    /// Readable only while the policy grants a network — see
    /// [`discovery::network_scoped_reads`]. Held separately from
    /// `readable_extra` because the network posture is set AFTER construction
    /// (`with_network`), so the decision cannot be taken in the constructor.
    network_scoped_readable: Vec<PathBuf>,
    /// Paths the child may `stat` but never read — see
    /// [`wcore_sandbox::SandboxManifest::fs_metadata_read_allow`] for the
    /// backend contract and [`discovery::libgit2_global_config_probes`] for the
    /// only thing that currently needs it.
    ///
    /// Held apart from `readable_extra` on purpose: these are NOT readable
    /// roots. Folding them in would hand the child the file's contents, which
    /// for `~/.gitconfig` means the operator's identity and any
    /// `[url … insteadOf]` rewrite they have configured.
    metadata_readable: Vec<PathBuf>,
    network: NetworkPolicy,
    cache_env: Vec<(String, String)>,
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
    /// The ONLY principal that can drive this session's shell is the local
    /// operator at their own keyboard — there is no channel/remote scope on the
    /// engine that owns this policy. See
    /// [`shell_requires_os_read_deny`](Self::shell_requires_os_read_deny) for
    /// what it buys and why it is not `secret_read_deny_required`'s inverse.
    ///
    /// FAIL-SAFE DEFAULT: `false` in every constructor. A policy only becomes a
    /// local-operator policy by an explicit
    /// [`with_shell_principal`](Self::with_shell_principal) at a seam that can
    /// see the channel scope and the execution floor, so a new construction path
    /// cannot acquire the relaxation by omission.
    local_operator_principal: bool,
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
        let scratch = scratch_dirs(WorkspaceTrust::Trusted);
        let cache_env = temp_env(&scratch);
        let mut writable_extra = scratch;
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
        let network_scoped_readable = network_scoped_reads();

        Self {
            root,
            trust: WorkspaceTrust::Trusted,
            writable_extra,
            readable_extra,
            network_scoped_readable,
            // Nothing to grant: `trusted_config_and_certificate_reads` already
            // gives this profile the CONTENTS of `~/.gitconfig`, so the
            // metadata channel would be redundant here.
            metadata_readable: Vec::new(),
            // #657: the bare constructor is fail-safe — network is seeded from
            // `default_bash_network_policy()`, an unconditional Deny.
            // Network egress is granted only for a GENUINELY-LOCAL session, and
            // that grant is applied at bootstrap via `with_network(Inherit)` gated
            // on `channel_tool_posture.is_none()` (see `local_bash_network`). A
            // channel-attached session — including `Full` posture — is a remote
            // sender and stays on this Deny default: it must not get a networked
            // shell by default (Overwatch ruling on #657, Sean-confirmed).
            network: crate::bash::default_bash_network_policy(),
            cache_env,
            authority_read_deny: Vec::new(),
            authority_write_deny: Vec::new(),
            deny_git_authority_env: false,
            delegated_scratch: None,
            // Genuinely-local Trusted default: no project-secret denial, so the
            // Bash read-deny-enforcement gate does not apply. `with_project_secret_deny`
            // flips this to true for a Full/remote session (#667).
            secret_read_deny_required: false,
            local_operator_principal: false,
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
        let mut cache_env: Vec<(String, String)> = CACHE_ENV_DIRS
            .iter()
            .map(|(var, sub)| {
                (
                    (*var).to_string(),
                    cache_root.join(sub).to_string_lossy().into_owned(),
                )
            })
            .collect();
        let readable_extra = contained_toolchain_read_dirs();
        let network_scoped_readable = Vec::new();
        let writable_extra = scratch_dirs(WorkspaceTrust::Contained);
        cache_env.extend(temp_env(&writable_extra));
        cache_env.extend(git_config_env(&cache_root));

        Self {
            root,
            trust: WorkspaceTrust::Contained,
            writable_extra,
            readable_extra,
            network_scoped_readable,
            metadata_readable: libgit2_global_config_probes(),
            // #657: a Contained (untrusted / remote `Workspace`) posture runs
            // potentially attacker-influenced content, so egress stays DENIED to
            // keep the exfil boundary tight. The operator's config-file
            // `[security] egress_allow` is the explicit escape hatch, applied at
            // bootstrap via `with_network(operator_bash_network(..))` — SEC-11
            // deleted the `WAYLAND_BASH_ALLOW_NETWORK` env lever that used to
            // fill that role from untrusted provenance.
            network: crate::bash::default_bash_network_policy(),
            cache_env,
            authority_read_deny: Vec::new(),
            authority_write_deny: Vec::new(),
            deny_git_authority_env: false,
            delegated_scratch: None,
            // Contained denies project secrets → Bash must be refused when the
            // backend can't enforce read-deny (else `cat .env` fails open).
            secret_read_deny_required: true,
            local_operator_principal: false,
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

        let readable_extra = contained_toolchain_read_dirs();
        let network_scoped_readable = Vec::new();
        let writable_extra = vec![scratch.clone()];
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
        // Same redirect `contained` applies, for the same reason: this profile
        // does not grant `$HOME/.gitconfig` either, and git opens it
        // unconditionally, so without this every `git` invocation inside a
        // delegated forge dies at exit 128 under seatbelt. `deny_git_authority_env`
        // strips an AMBIENT `GIT_CONFIG*` out of the passthrough; this is the
        // policy's own value and is applied after that filter, on purpose.
        cache_env.extend(git_config_env(&scratch.join("cache")));

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
            network_scoped_readable,
            metadata_readable: libgit2_global_config_probes(),
            network: crate::bash::default_bash_network_policy(),
            cache_env,
            authority_read_deny: protected.clone(),
            authority_write_deny: protected,
            deny_git_authority_env: true,
            delegated_scratch: Some(scratch),
            secret_read_deny_required: true,
            // A delegated mutation is issued BY an orchestrator, not typed by the
            // operator, so it is never a local-operator principal.
            local_operator_principal: false,
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
        // Network-scoped reads are withheld from a Deny-network child: they
        // describe the network it does not have, and naming the host's DNS
        // servers and search domains is the whole of what they leak.
        // `AllowHosts` still needs to resolve names, so only `Deny` withholds.
        if !matches!(self.network, NetworkPolicy::Deny) {
            v.extend(self.network_scoped_readable.iter().cloned());
        }
        v.extend(self.session_read_grants.read().iter().cloned());
        v.sort();
        v.dedup();
        v
    }
    /// Paths the child may `stat` but never read the contents of.
    ///
    /// Deliberately NOT merged into [`Self::readable_roots`] — see the field
    /// doc. Also deliberately not filtered by `exists()`: under seatbelt an
    /// absent path is EPERM too, so a host with no `~/.gitconfig` needs the
    /// grant just as much as one that has it.
    pub fn metadata_readable_roots(&self) -> Vec<PathBuf> {
        self.metadata_readable.clone()
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
    /// denial (`secret_deny_paths_dynamic()`) that `Contained` applies, so a
    /// `Full`-posture channel / remote session's `Bash` OS-sandbox refuses to
    /// read the workspace's own secrets. A GENUINELY-LOCAL keyboard session
    /// (no channel posture) does NOT call this — the operator may read their
    /// own `.env`. Complements the `SecretDenyFs` read-path guard installed for
    /// the same sessions at bootstrap.
    ///
    /// Setting the flag IS the whole opt-in: `secret_deny_paths_dynamic()` —
    /// the only thing `bash.rs` feeds to the OS sandbox — keys the project
    /// walk off `secret_read_deny_required`. Idempotent because it is a bool.
    pub fn with_project_secret_deny(mut self) -> Self {
        // #667 F2: this Trusted policy now denies project secrets, so its `Bash`
        // must also be refused when the backend can't enforce read-deny.
        self.secret_read_deny_required = true;
        self
    }

    /// THE ONE ANSWER to "may this policy's shell be relaxed onto a backend
    /// that cannot enforce OS secret-read-deny?" — i.e. is the only principal
    /// who can drive it the local operator at their own keyboard?
    ///
    /// EVERY production path that builds a shell-bearing `WorkspacePolicy` must
    /// call this rather than deciding for itself. There are two such paths —
    /// `AgentBootstrap::build` (the session) and `wcore_cli::sandbox_cmd`
    /// (`wayland-core sandbox exec`) — and they used to disagree, because only
    /// the first one had the carve-out at all: `sandbox exec` refused every
    /// shell on the Windows relaxed default while the session beside it worked.
    /// Two independent answers to one question is how they drifted, so there is
    /// now one answer. `sandbox_exec_principal_parity` fails if they diverge.
    ///
    /// The two facts stay the CALLER's to supply, because they come from
    /// different places and only the caller can know them:
    ///
    /// * `channel_posture_present` — is a channel/remote sender able to reach
    ///   this shell? `Some(ChannelToolScope)` on the engine for a session;
    ///   structurally `false` for `sandbox exec`, which is reachable only from
    ///   this host's argv (`TopCmd::Sandbox`) and has no channel, protocol or
    ///   slash route.
    /// * `managed_execution_floor` — is an administrator-imposed Managed policy
    ///   installed? That floor is not this relaxation's to lift, on either path.
    ///
    /// Neither input can be selected by repository content.
    ///
    /// Effect when the answer is "the local operator":
    /// [`shell_requires_os_read_deny`](Self::shell_requires_os_read_deny)
    /// goes false, so `bash.rs` stops refusing the shell on a backend that
    /// cannot enforce OS-level secret-read-deny. Nothing else moves —
    /// `secret_read_deny_required` is untouched, so the OS deny LIST
    /// ([`secret_deny_paths_dynamic`](Self::secret_deny_paths_dynamic)) is still
    /// computed and still handed to the backend, and a backend that CAN enforce
    /// it still does. The tool-layer `SecretDenyFs` on Read/Write/Edit is a
    /// different guard entirely and is unaffected.
    #[must_use]
    pub fn with_shell_principal(
        self,
        channel_posture_present: bool,
        managed_execution_floor: bool,
    ) -> Self {
        if channel_posture_present || managed_execution_floor {
            self
        } else {
            self.with_local_operator_principal()
        }
    }

    /// Unconditionally mark this policy's shell principal as the local operator.
    ///
    /// Production code must NOT call this — call
    /// [`with_shell_principal`](Self::with_shell_principal), which is the one
    /// place the decision is made. This exists so tests can construct a
    /// local-operator policy directly without reconstructing a whole engine.
    #[must_use]
    pub fn with_local_operator_principal(mut self) -> Self {
        self.local_operator_principal = true;
        self
    }

    /// True when this policy's shell may only be driven by the local operator.
    #[must_use]
    pub fn local_operator_principal(&self) -> bool {
        self.local_operator_principal
    }

    /// THE exec-time shell gate predicate: `Bash` must be REFUSED when this is
    /// true and the active backend neither enforces read-deny nor is an
    /// operator-requested containment bypass.
    ///
    /// It is `secret_read_deny_required` AND NOT `local_operator_principal`,
    /// because the two flags answer different questions:
    ///
    /// * `secret_read_deny_required` — *does this policy's confidentiality story
    ///   depend on the OS enforcing `fs_read_deny`?* True for `Contained` and for
    ///   any `Trusted` policy that opted into project-secret denial.
    /// * `local_operator_principal` — *who can drive the shell?* When the answer
    ///   is "only the human who launched this process", the confidentiality the
    ///   deny list protects is that human's own, from that human. The refusal
    ///   buys nothing and costs the entire shell: it fires on every fresh clone
    ///   (untrusted workspace ⇒ `contained`), and the product's own printed
    ///   remedy — `--trust-workspace` — hands back a `trusted_local` policy with
    ///   `secret_read_deny_required == false` and therefore the SAME uncontained
    ///   shell, with no extra authority and no extra OS enforcement. A gate whose
    ///   documented one-command bypass grants the identical capability is a
    ///   usability cost, not a boundary.
    ///
    /// The refusal is UNCHANGED for every principal that is not the local
    /// operator: channel/remote sessions of any posture, Managed execution
    /// policy, and delegated orchestrator mutations all leave
    /// `local_operator_principal` false and are still refused.
    #[must_use]
    pub fn shell_requires_os_read_deny(&self) -> bool {
        self.secret_read_deny_required && !self.local_operator_principal
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
    /// This is the SOLE source of `manifest.fs_read_deny` (`bash.rs`). It
    /// re-walks the workspace for project-committed secrets on every exec, so a
    /// secret CREATED AFTER bootstrap (a pulled `*.pem`, a generated
    /// `terraform.tfstate`) is denied on the very next Bash command. That closed
    /// the TOCTOU gap against the frozen construction-time list `bash.rs` used
    /// before #234; the frozen list has since been DELETED, because it had no
    /// remaining production reader and a stale `pub` deny-list accessor sitting
    /// next to this one is a trap for the next caller. Its removal also took a
    /// full recursive no-prune walk of the workspace off the TUI boot path,
    /// where it blocked first paint inside `splash_while` (see
    /// `project_committed_secrets`). The in-process file tools enforce
    /// separately and per-access via [`is_project_secret`](Self::is_project_secret).
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

    /// #922 R1: the OS read-deny list, computed only for a backend that will
    /// actually apply it.
    ///
    /// `manifest.fs_read_deny` has exactly one producer
    /// ([`secret_deny_paths_dynamic`](Self::secret_deny_paths_dynamic)) and the
    /// backends are its only enforcing consumer. A backend whose
    /// `SandboxBackend::enforces_read_deny()` is `false` — the trait default,
    /// which the Windows session default `windows_job_object` deliberately
    /// keeps — takes no filesystem action on the field at all. Producing it for
    /// such a backend is a full no-prune walk of the workspace whose only
    /// result is dropped: measured on SeanDesktop at 76,367 ms against a real
    /// user profile versus 175 ms against an empty directory (#922).
    ///
    /// This is NOT a relaxation of a security profile — the precedent this
    /// project set with "enforces_read_deny is liveness, not policy" is about
    /// using this predicate to WEAKEN a profile, and that is not what happens
    /// here. A `false` answer is the definition of "this field is discarded",
    /// so skipping its computation is observationally identical. The gate also
    /// fails in the safe direction: every enforcing backend
    /// (`bwrap`/`sandbox_exec`/live `docker`) hardcodes `true`, and
    /// `AppContainerBackend` derives its answer from a monotone probe that
    /// answers `true` while unsettled — an unknown answer therefore
    /// over-reports enforcement and we still walk (stale-POSITIVE, an
    /// availability cost, never a leak). That single assumption is pinned by
    /// `crates/wcore-sandbox/tests/enforces_read_deny_pairing.rs` (A2).
    ///
    /// Note the layering: this gate gets to exist only because it guards the
    /// OS-enforced list. The IN-PROCESS file-tool predicate
    /// ([`is_project_secret`](Self::is_project_secret), via
    /// `vfs::SecretDenyFs`) is enforced by this process and must NEVER be
    /// routed through a backend capability — pinned by
    /// `crates/wcore-tools/tests/vfs_secret_deny_backend_independent.rs` (A7).
    pub fn secret_deny_paths_for_backend(&self, backend_enforces_read_deny: bool) -> Vec<PathBuf> {
        if !backend_enforces_read_deny {
            // The backend discards this field; producing it is pure cost.
            return Vec::new();
        }
        self.secret_deny_paths_dynamic()
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
/// The one credential-file name predicate in the crate. `Read`/`SecretDenyFs`
/// reach it via [`WorkspacePolicy::is_secret_path`]; `grep_policy` (SR-05) uses
/// it directly, because Grep has no policy instance and must not grow a second,
/// divergent copy of this list.
pub(crate) fn is_secret_path_static(path: &Path) -> bool {
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
    //
    // Pinned by `tests::no_prune_survives_the_922_backend_gate` (#922 A4), which
    // asserts a secret under `node_modules/` and under `target/` is still in the
    // list. #922's fix declines to COMPUTE this walk on a backend that discards
    // it; it must never become a reason to PRUNE it, because a prune is a
    // permanent stale-negative against the in-process predicate above.
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

/// Leaf name of the bounded scratch tree, replacing a grant over the whole host
/// temp directory.
const SCRATCH_ROOT: &str = "wayland-scratch";

/// The writable scratch grant handed to a sandboxed session.
///
/// Previously this was the entire host temp tree (`vec![canon(temp_dir())]`).
/// On Windows every writable root is materialized as an inheritable ACE via
/// `SetNamedSecurityInfoW` per spawn and revoked afterwards, so granting
/// `%TEMP%` is both O(subtree) in cost and enormous in blast radius: a crash
/// between grant and revoke strands an ACE on a directory shared with every
/// other application on the machine. It is also far more authority than a
/// sandboxed child needs — it could read and rewrite any other process's temp
/// state, including files a more privileged program is mid-way through writing.
///
/// **The scratch dir is keyed by trust.** One fixed name shared by
/// [`WorkspacePolicy::trusted_local`] and [`WorkspacePolicy::contained`] would
/// hand an untrusted/remote `Contained` session a writable host directory that
/// a `Trusted` local session also writes to and reads back — a trust-crossing
/// channel created by the narrowing itself.
///
/// Returns an EMPTY grant, never a fallback to `%TEMP%`, when the directory
/// cannot be established: failing closed costs a session its scratch space,
/// whereas failing open silently restores the defect this function exists to
/// remove.
fn scratch_dirs(trust: WorkspaceTrust) -> Vec<PathBuf> {
    match scratch_dir(trust) {
        Some(dir) => vec![dir],
        None => Vec::new(),
    }
}

/// `TMPDIR`/`TMP`/`TEMP` pointed INTO the session's own writable scratch grant.
///
/// SEC-06 / SEC-10 made the bwrap backend remount every ungranted filesystem
/// read-only, and the host `/tmp` is ungranted. Left pointing at the host temp
/// directory, these vars turn every temp-using tool into a failure — and the
/// dangerous ones do not fail loudly. Measured on hetzner-dsm under
/// `trusted_local` and `contained` before this redirect existed:
/// `mktemp` → "Read-only file system" (exit 1, honest), but
/// `seq 1 200000 | sort -R | wc -l` → prints `0`, **exit 0**, `is_error=false`.
/// A silently wrong answer reported as success is exactly the defect the
/// read-only remount was added to remove, recreated at a different path.
///
/// [`WorkspacePolicy::delegated_mutation`] already does this with its private
/// scratch root; this is the same mechanism for the two profiles a real user
/// actually gets. It grants NO new authority — `scratch_dirs` already put this
/// directory in `writable_extra`, so it is a writable root either way, and it
/// is per-uid, per-trust and ownership-verified (see [`scratch_dir`]). Because
/// every write grant is its own bind mount and `--remount-ro` does not touch
/// submounts, it stays writable underneath a read-only `/tmp`.
///
/// Empty when the scratch grant could not be established: with no writable
/// scratch to point at, redirecting would only relocate the failure.
/// `GIT_CONFIG_GLOBAL` / `GIT_CONFIG_SYSTEM` pointed into the workspace's own
/// cache root, for the same reason `CARGO_HOME` and `npm_config_cache` are.
///
/// Under the `contained` profile — **the profile a workspace gets by default,
/// because [`EffectiveWorkspaceTrust`] starts untrusted** — `$HOME/.gitconfig`
/// is deliberately not granted. git reads it unconditionally at startup, so on
/// macOS seatbelt the result was that `git` did not work AT ALL: measured on
/// Darwin 25.3.0, `git init` exited 128 with
/// `fatal: unable to access '/Users/<me>/.gitconfig': Operation not permitted`,
/// and so did every other subcommand.
///
/// The fix is to stop git reading the host file rather than to grant it. This
/// REMOVES authority: the sandboxed child no longer sees the operator's global
/// git configuration at all, which also means a `[url … insteadOf]` rewrite
/// carrying an embedded credential can no longer be applied on behalf of
/// untrusted workspace content. Both variables are absolute file paths (git
/// requires that), inside `<root>/.wcache`, which is already a writable root —
/// so `git config --global` inside the sandbox lands in a real file scoped to
/// the workspace instead of being silently discarded.
///
/// This does NOT fix `cargo new`, and that is not an oversight: cargo's VCS
/// init goes through libgit2, which computes the global config path from
/// `$HOME` and ignores `GIT_CONFIG_GLOBAL` entirely — measured, it still fails
/// with `failed to stat '/Users/<me>/.gitconfig'; class=Config (7)`. Closing
/// that needs a metadata-only grant on the file, which needs a manifest channel
/// that does not exist yet; see the lane report rather than a silent widening.
fn git_config_env(cache_root: &Path) -> Vec<(String, String)> {
    let git = cache_root.join("git");
    [
        ("GIT_CONFIG_GLOBAL", "config"),
        ("GIT_CONFIG_SYSTEM", "system"),
    ]
    .into_iter()
    .map(|(var, file)| {
        (
            var.to_owned(),
            git.join(file).to_string_lossy().into_owned(),
        )
    })
    .collect()
}

fn temp_env(scratch: &[PathBuf]) -> Vec<(String, String)> {
    let Some(dir) = scratch.first() else {
        return Vec::new();
    };
    ["TMPDIR", "TMP", "TEMP"]
        .into_iter()
        .map(|var| (var.to_owned(), dir.to_string_lossy().into_owned()))
        .collect()
}

/// SAFETY: `getuid` is a POSIX call that cannot fail, takes no arguments and
/// touches no caller-owned memory. It is `unsafe` only because it is `extern`.
#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

fn scratch_dir(trust: WorkspaceTrust) -> Option<PathBuf> {
    let mut dir = std::env::temp_dir();
    // The uid goes in the TOP component on unix, not a subdirectory: there
    // `temp_dir()` is the shared, world-writable `/tmp`, and a shared parent
    // would mean whichever user created it first owns the permissions for
    // everyone else.
    #[cfg(unix)]
    dir.push(format!("{SCRATCH_ROOT}-u{}", current_uid()));
    // `%TEMP%` is already per-user on Windows.
    #[cfg(not(unix))]
    dir.push(SCRATCH_ROOT);
    dir.push(match trust {
        WorkspaceTrust::Trusted => "trusted",
        WorkspaceTrust::Contained => "contained",
    });

    std::fs::create_dir_all(&dir).ok()?;

    // `/tmp` is world-writable, so another user can pre-create this name — as a
    // symlink to somewhere valuable, or as a directory they retain write access
    // to. `create_dir_all` follows symlinks and succeeds in both cases. Verify
    // we got a real directory that we own before granting a write ACE to it.
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != current_uid() {
            return None;
        }
    }

    Some(canon(dir))
}

/// #657 (Overwatch ruling, Sean-confirmed): the Bash network posture for a
/// `Trusted` workspace is `Inherit` (egress ON — npm/pip/cargo/brew installs,
/// curl, git fetch just work) ONLY for a GENUINELY-LOCAL session: one with no
/// channel posture attached (local CLI / TUI / json-stream / ACP / desktop).
///
/// A channel-attached session — INCLUDING `Full` posture — is a remote sender.
/// It stays on the pre-#657 lockdown: `default_bash_network_policy()`, which is
/// an unconditional Deny. A remote-triggered context does not get a networked
/// shell; if a real remote-networked-shell use case appears, it becomes a
/// deliberate per-channel opt-in, not the default.
pub fn local_bash_network(has_channel_posture: bool) -> NetworkPolicy {
    if has_channel_posture {
        crate::bash::default_bash_network_policy()
    } else {
        NetworkPolicy::Inherit
    }
}

/// SEC-13 — the Bash network posture for a SANDBOXED (`contained`) session,
/// decided by the operator's TRUSTED `[security] allow_sandboxed_shell_network`.
///
/// The W2/W3 conformance gate measured the polarity backwards on Linux: a bare
/// `WAYLAND_BASH_ALLOW_NETWORK=1` in the environment re-opened the sandboxed
/// shell (`accept_count=1` on a driver-owned listener) while the operator's own
/// trusted config recorded `accept_count=0`. Untrusted provenance raised the
/// boundary; trusted provenance had no lever at all.
///
/// The env lever is gone (see [`crate::bash::default_bash_network_policy`]) and
/// this is its replacement, matching the posture `SecurityConfig::enabled`
/// already documents: **config-file only, read from the trusted layer**. The
/// `[security]` arm of `merge_config_files_with_trust` takes this field from the
/// GLOBAL layer alone, so a project file — which travels with a cloned
/// repository — cannot mint the grant.
///
/// It is deliberately **not** derived from `[security] egress_allow`. The first
/// draft of this fix was, and that was a worse trade than the defect it closed:
/// `egress_allow` is a per-host permit for the in-process HTTP gate, the strict
/// branch is selected by `!workspace_trust.is_trusted()` — the DEFAULT for any
/// repo the operator has not fingerprint-trusted — and no sandbox backend in
/// this repo has a host/DNS gate for an arbitrary shell (bwrap, sandbox-exec,
/// AppContainer and Docker all reject [`NetworkPolicy::AllowHosts`]), so the
/// enforceable shell grant is all-or-nothing. Measured on the first draft:
/// `egress_allow = ["docs.rs"]` opened a connection to `127.0.0.1:44755`, a host
/// the operator never listed. Permitting one host for one subsystem must not
/// hand an untrusted repository's shell arbitrary outbound TCP.
///
/// GRANULARITY, stated plainly: `true` yields [`NetworkPolicy::Inherit`] — the
/// whole host network — and that is logged at `warn` every time it bites rather
/// than happening quietly. There is no narrower enforceable option today.
pub fn operator_bash_network(allow_sandboxed_shell_network: bool) -> NetworkPolicy {
    if !allow_sandboxed_shell_network {
        return NetworkPolicy::Deny;
    }
    tracing::warn!(
        target: "wcore_tools::workspace_policy",
        "[security] allow_sandboxed_shell_network = true, so the sandboxed shell is \
         granted network access. No sandbox backend can filter an arbitrary shell's \
         egress by host, so this grant is the WHOLE host network. Set it back to \
         false to keep the shell offline."
    );
    NetworkPolicy::Inherit
}

mod discovery;
use discovery::{
    capability_roots, contained_toolchain_read_dirs, detect_developer_capabilities,
    libgit2_global_config_probes, network_scoped_reads, trusted_config_and_certificate_reads,
};

#[cfg(test)]
mod tests;
