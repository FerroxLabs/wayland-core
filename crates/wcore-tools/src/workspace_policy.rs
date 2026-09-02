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
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use thiserror::Error;
use wcore_protocol::PathGrantSink;
use wcore_sandbox::manifest::NetworkPolicy;
use wcore_types::workspace_trust::DeveloperCapability;

/// Upper bound on standing read grants in one session. A grant is minted by a
/// person clicking "always allow this folder", so a healthy session mints a
/// handful. The cap exists so a looping agent that keeps re-asking cannot grow
/// the reachable set without bound.
const MAX_SESSION_READ_GRANTS: usize = 64;

const SECRET_SUFFIXES: &[&str] = &[
    "/.env",
    "/.git/config",
    "/.hg/hgrc",
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

/// Workspace-relative directories that stay READ-ONLY to the in-process file
/// tools in every posture — see
/// [`WorkspacePolicy::is_repo_control_path`].
///
/// `.git` is listed WHOLE rather than as the two leaves
/// (`hooks/`, `config`) already carried by [`SECRET_SUFFIXES`]: git's
/// execute-on-next-command surface is not confined to `hooks/` — `config`
/// alone reaches it through `core.fsmonitor`, `core.sshCommand`,
/// `diff.*.textconv` and `filter.*.clean/smudge`, and `.git/info/attributes`
/// selects those filters per path. Enumerating the reachable keys is a losing
/// game against a format git keeps extending, so the directory is the unit.
///
/// This is a FILE-TOOL deny, and `Bash` is deliberately not held to the same
/// line: `git commit`, `git add` and every other porcelain verb write `.git`
/// freely, because confining them would break committing outright. The
/// asymmetry is the point — `Bash` is an explicit request to run a program,
/// while `Write`/`Edit` are the low-friction surface a prompt injection reaches
/// for.
///
/// #693 narrowed that asymmetry without removing it. The command floor
/// (`wcore_config::command_floor`, called first on all four `BashTool` entry
/// points) refuses a shell command that NAMES `.git/hooks` or `.git/config` —
/// the two execute-on-next-command surfaces — while every porcelain write to
/// `.git` stays allowed. It matches path tokens in the command, so it is NOT
/// the sandbox-expressed write-deny that `SandboxManifest` cannot carry; it is
/// a floor underneath the sandbox, not a substitute for one.
const REPO_CONTROL_DIRS: &[&str] = &[".git", ".wayland-core"];

/// Secret file EXTENSIONS, matched case-insensitively on the effective final
/// component. `keystore` / `jks` (Java + Android signing stores) arrived with
/// core#323 — see [`SECRET_BASENAMES`] for why they were somewhere else.
const SECRET_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pfx", "tfstate", "keystore", "jks"];

/// Secret basenames matched on the final path component, case-insensitively.
///
/// ONE LIST, ONE OWNER (FerroxLabs/wayland-core#323). The eleven names below
/// the SSH keys used to live in a SECOND denylist, `wcore-cli`'s `@`-attach
/// guard, and the two drifted apart in both directions. #323's first cut taught
/// the `@` surface to consult BOTH lists — which closed the `@` half and left
/// the more dangerous half open: a user typing `@.pgpass` was refused while the
/// MODEL could read the same file through `Read` / `Grep` / `Bash cat`, because
/// those consult this predicate alone. Two lists that must agree drift again,
/// so the file-name rules moved HERE and the `@` guard now delegates. The only
/// thing it still contributes is a leading separator, because a user types a
/// relative path and the fragment rules below need an anchored one.
const SECRET_BASENAMES: &[&str] = &[
    // SSH private keys.
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    // core#323 — folded in from the `@`-attach guard.
    ".envrc",
    ".pgpass",
    "credentials",
    "credentials.json",
    "secrets.json",
    "secrets.yaml",
    "secrets.yml",
];

/// File-name SUFFIXES that mark a secret whatever the stem: the conventional
/// spelling of a NAMED SSH key (`deploy_rsa`, `ci_ed25519`), which
/// [`SECRET_BASENAMES`] cannot express because it matches the whole name.
/// core#323, folded in from the `@`-attach guard alongside the basenames above.
const SECRET_NAME_SUFFIXES: &[&str] = &["_rsa", "_ed25519"];

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
    session_path_grants: Arc<RwLock<Vec<SessionPathGrant>>>,
    /// #1104 — the name of the OS sandbox backend that CONFINES this session's
    /// filesystem, when one does.
    ///
    /// FAIL-SAFE DEFAULT: `None` in every constructor. A policy only learns it
    /// is filesystem-confined through an explicit
    /// [`with_filesystem_confinement`](Self::with_filesystem_confinement) at
    /// the one seam that can see the selected backend, so a new construction
    /// path cannot acquire write-grantability by omission.
    ///
    /// It gates WRITE grants only. On a backend that answers
    /// `confines_filesystem() == false` — today the Windows job-object default
    /// — a shell command in this session can already create or overwrite files
    /// anywhere this user account can. A "write access to this one folder"
    /// grant there would be a button describing a boundary that does not
    /// exist, so it is refused rather than faked. Read grants are unaffected:
    /// they widen the IN-PROCESS jail, which is real on every platform.
    fs_confinement_backend: Option<String>,
    /// #1111 — the memoised exec-path deny set, with the stamp that proves it
    /// is still what a fresh walk would produce. Interior-mutable and shared by
    /// `Arc` for the same reason `session_path_grants` is: `bash.rs` holds the
    /// policy behind an `Arc` and cannot replace it between executions.
    deny_cache: Arc<RwLock<Option<DenyCache>>>,
    /// #1111 — how many times this policy has actually recomputed the dynamic
    /// deny set from the filesystem. Read by
    /// [`secret_deny_walk_count`](Self::secret_deny_walk_count); it is the
    /// injected counter #1111 asks the repeated-walk assertion to be made with,
    /// so the grade does not rest on a wall clock.
    deny_walks: Arc<AtomicU64>,
    /// FerroxLabs/wayland-core#376 — the memoised arm-2 VCS store list, with the
    /// stamp that proves it is still what a fresh scan would produce.
    /// Interior-mutable and `Arc`-shared for the reason `deny_cache` is: the
    /// policy is held behind an `Arc` by every consumer and cannot be replaced.
    vcs_store_cache: Arc<RwLock<Option<VcsStoreCache>>>,
    /// #376 c3 — the injected counters the per-operation cost is graded with. A
    /// wall clock cannot tell a skipped scan from a fast one, and this guard
    /// runs on every Read/exists/list/metadata of every sub-agent.
    guard_counters: Arc<GuardCounters>,
    /// FerroxLabs/wayland-core#390/#394 — arm 4's discovered nested store set,
    /// computed at most ONCE per policy and then never touched again.
    ///
    /// Computed once and then answered by prefix at ZERO filesystem cost, with
    /// a witness set that is O(nested checkouts) rather than O(directories) —
    /// which is the whole point. Which directories under the root DECLARE a
    /// store is a whole-tree fact, and the shape #398 was filed about is a memo
    /// of that fact revalidating one witness per DESCENDED DIRECTORY on every
    /// guard. This one stamps only the DECLARATION SITES the walk actually read
    /// (a nested gitfile, a `commondir`, an `objects/info/alternates`, a store
    /// leaf that may be re-pointed), never the directories it merely walked
    /// through, and revalidates them only on the branch that is about to ADMIT.
    /// A workspace with no nested checkout therefore has an EMPTY witness set
    /// and pays nothing at all, which is what holds #398 c1's slope at zero and
    /// #398 c2's three warm probes. See
    /// [`nested_stores_cover`](Self::nested_stores_cover) and
    /// [`nested_declarations_moved`](Self::nested_declarations_moved).
    nested_stores: Arc<RwLock<Option<NestedStoreCache>>>,
}

/// #376 c3 — what one `SecretDenyFs` guard actually costs, counted rather than
/// timed.
#[derive(Debug, Default)]
pub(crate) struct GuardCounters {
    /// Path resolutions (`canon_existing_ancestor`) performed by the guard
    /// predicates. One per guard is the target; it was two before the
    /// predicates were given a shared resolved path.
    resolves: AtomicU64,
    /// Full rebuilds of the arm-2 store list from the filesystem.
    scans: AtomicU64,
    /// Filesystem probes (`exists` / `canonicalize` / `symlink_metadata` /
    /// `read_to_string`) charged by scans AND by cache revalidations.
    probes: AtomicU64,
    /// Arm-4 nested-discovery walks. Counted APART from `scans` because the two
    /// mean different things: `scans` is the arm-2 rebuild whose return to the
    /// common path is core#376's regression, while this one is a per-policy
    /// one-off by construction and asserting it stays at 1 is what proves that.
    nested_walks: AtomicU64,
}

/// #376 — one memoised store list plus everything needed to decide whether it
/// is stale.
#[derive(Debug)]
struct VcsStoreCache {
    /// The instant the scan that produced `stores` started.
    stamped_at: SystemTime,
    /// Every path whose state decided the answer, with the modification time
    /// the scan saw (`None` when the path was absent).
    ///
    /// Deliberately the DIRECTORY that owns each decision rather than the
    /// decided path itself, wherever one exists: whether `<root>/.git/objects`
    /// is present, absent or re-pointed is settled by `<root>/.git`'s mtime, so
    /// one stamp covers all three of `.git`'s store leaves instead of three.
    /// The exception is a file whose CONTENT is read (`objects/info/alternates`,
    /// a gitfile, a `commondir`) — content changes leave the parent untouched,
    /// so those are stamped in their own right.
    witnesses: Vec<(PathBuf, Option<SystemTime>)>,
    stores: Vec<PathBuf>,
}

/// FerroxLabs/wayland-core#406 — arm 4's discovered store set plus the
/// DECLARATION SITES whose state decided it.
///
/// Deliberately NOT the directories the walk descended: that set is
/// O(workspace) and re-`stat`ing it per guard is the measured regression #398
/// records. The declaration sites are the files and store leaves the walk
/// actually READ — one gitfile, one `commondir`, one `alternates` and three
/// store leaves per nested checkout — so the set is O(nested checkouts) and is
/// EMPTY for a workspace that has none.
#[derive(Debug)]
struct NestedStoreCache {
    /// The instant the walk that produced `stores` started.
    stamped_at: SystemTime,
    /// Each declaration site with the modification time the walk saw (`None`
    /// when it was absent — which is the stamp that catches an `alternates`
    /// file being CREATED after the walk, the #406 residual).
    witnesses: Vec<(PathBuf, Option<SystemTime>)>,
    stores: Vec<PathBuf>,
}

/// #1111 — one memoised deny set plus everything needed to decide whether it is
/// stale.
#[derive(Debug)]
struct DenyCache {
    /// Everything about the POLICY (as opposed to the tree) that changes the
    /// answer — see [`WorkspacePolicy::deny_cache_key`]. A difference here is an
    /// outright miss and no directory is stat'ed.
    key: u64,
    /// The instant the walk that produced `paths` started.
    stamped_at: SystemTime,
    /// Every directory that walk descended into, with its modification time.
    dirs: Vec<(PathBuf, SystemTime)>,
    paths: Vec<PathBuf>,
}

/// #1111 — a tree with more directories than this keeps today's per-exec walk
/// rather than growing an unbounded stamp. Remembering a directory costs a
/// `PathBuf` and revalidating it costs one `stat`, so past some size the memo
/// stops paying for itself and starts costing memory instead.
const DENY_CACHE_MAX_DIRS: usize = 100_000;

/// #1145 - how far a directory mtime must lag the instant a walk started before
/// that walk's answer may be trusted, on a filesystem that stamps SUB-SECOND
/// modification times.
///
/// `stamped_at` is a `SystemTime::now()` and is therefore fine-grained, but a
/// directory mtime is stamped by the kernel from a COARSE clock: measured on
/// this project's Linux build host, ext4 and tmpfs both report exactly one
/// jiffy of granularity (1.000010 ms at `CONFIG_HZ=1000`), and 2000 of 2000
/// post-walk writes produced an mtime STRICTLY LESS than a `stamped_at` taken
/// moments before. A bare `now >= stamped_at` test can therefore essentially
/// never fire, which is what left the same-tick write in #1145 invisible.
///
/// A change made within one tick of the walk cannot be witnessed by an mtime at
/// all, so the only sound answer is to distrust the memo for that long. 20 ms is
/// 20x the granularity measured here and 2x the coarsest jiffy any supported
/// Linux kernel uses (10 ms at `CONFIG_HZ=100`); APFS and NTFS stamp finer
/// still. It costs one extra walk per `Bash` execution that starts within 20 ms
/// of the workspace changing.
const SUBSECOND_MTIME_GRANULARITY: std::time::Duration = std::time::Duration::from_millis(20);

/// #1145 - the same slack for a filesystem that stamps WHOLE SECONDS: HFS+,
/// FAT/exFAT (two-second resolution), and older NFS servers. 20 ms would be
/// meaningless there - the tick such a filesystem can hide a write inside is a
/// thousand times longer.
const WHOLE_SECOND_MTIME_GRANULARITY: std::time::Duration = std::time::Duration::from_secs(2);

/// #667/#1118 — THE shell-principal predicate, in one place.
///
/// True when the ONLY principal that can drive a shell built from this decision
/// is the local operator at their own keyboard. Read directly by seams that must
/// derive a principal without building a policy first (a one-shot CLI driver's
/// spawner); [`WorkspacePolicy::with_shell_principal`] is the same question
/// asked of a policy, so the two cannot drift.
#[must_use]
pub fn local_operator_shell_principal(
    channel_posture_present: bool,
    managed_execution_floor: bool,
) -> bool {
    !(channel_posture_present || managed_execution_floor)
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

/// A write target that the session could create but could never read back.
///
/// FerroxLabs/wayland#1097. Write authority and read authority are enforced by
/// different mechanisms, so a location can be writable and unreadable at the
/// same time — and the asymmetry only reveals itself at the END of the work,
/// when the produced path is handed over and the read is refused. This is the
/// refusal that turns that dead end into a write-time error naming the reason.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{} is outside this session's readable roots", path.display())]
pub struct WriteTargetNotReadable {
    pub path: PathBuf,
}

/// One standing path grant held by a session.
///
/// Capability-derived roots (`grant_session_capability`) and host path grants
/// (`grant_session_read_root`) share ONE store, because they answer the same
/// question — what may this session reach — and two stores would be two
/// answers. They differ in whether they carry an id to revoke by, and in the
/// ACCESS they confer.
///
/// FerroxLabs/wayland#1104. `write` is a strictly narrower subset of the same
/// store rather than a second list, so the invariant "a write grant implies a
/// read grant; a read grant NEVER implies write" is structural: every read
/// consumer reads the whole store and is correct by construction, and exactly
/// ONE place ([`WorkspacePolicy::writable_roots`]) filters on `write`. Two
/// stores would need every read consumer to remember to union them, and a
/// forgotten union is a grant that silently does not work — the failure mode
/// the read grant already shipped once (`SandboxedFs` vs `readable_roots`).
#[derive(Debug, Clone)]
pub struct SessionPathGrant {
    pub root: PathBuf,
    /// Host-supplied id, for revocation. `None` for capability-derived roots,
    /// which are withdrawn only by ending the session.
    pub id: Option<String>,
    /// Wall-clock expiry. `None` means process lifetime.
    pub expires_at: Option<SystemTime>,
    /// Does this grant confer WRITE as well as read?
    ///
    /// FAIL-SAFE: `false` is the only value any read-grant path constructs.
    /// A `true` here is reachable solely through
    /// [`WorkspacePolicy::check_write_grantable`], which applies every read
    /// refusal PLUS the write-only ones.
    pub write: bool,
}

impl SessionPathGrant {
    fn is_live(&self, now: SystemTime) -> bool {
        self.expires_at.is_none_or(|deadline| now < deadline)
    }

    /// Does this grant confer at least `write` access, right now?
    fn confers(&self, now: SystemTime, write: bool) -> bool {
        self.is_live(now) && (!write || self.write)
    }
}

/// Why a standing folder grant (`ApprovalScope::AlwaysPath`) was refused.
///
/// Every variant is shown to the user verbatim: a grant that is dropped
/// silently looks to them like the "always allow this folder" button did not
/// work, and they will click it again.
#[derive(Debug, Error)]
pub enum PathGrantError {
    #[error("folder grants require a local session at your own keyboard")]
    RequiresLocalOperator,
    #[error("folder access could not be resolved: {0}")]
    Resolve(#[from] std::io::Error),
    #[error("{0} has no parent directory")]
    NoParent(PathBuf),
    #[error("the filesystem root cannot be granted")]
    FilesystemRoot,
    #[error("{0} contains your home directory — grant the specific folder instead")]
    TooBroad(PathBuf),
    #[error("{0} overlaps a credential store")]
    CredentialPath(PathBuf),
    #[error("{0} is a protected secret path")]
    SecretPath(PathBuf),
    // #1104 — the WRITE-only refusals. Everything above applies to a write
    // grant too; these apply ONLY to write, and are what makes it the stricter
    // grant rather than the same grant with a flag set.
    #[error(
        "write access needs an OS sandbox that confines the filesystem, and this session's does \
         not — a Bash command here can already create or overwrite files anywhere your account \
         can, so a write grant on {0} would name a boundary that does not exist. Read access is \
         still grantable."
    )]
    WriteRequiresConfinedBackend(PathBuf),
    #[error(
        "{0} would let this session replace a program you could run later — grant read access \
         instead, or pick a folder that holds no executables"
    )]
    WriteRootExecutable(PathBuf),
    #[error("{0} overlaps a location whose contents are run automatically")]
    WriteRootAutoRun(PathBuf),
    #[error("{0} holds a secret file, which a write grant could overwrite or replace")]
    WriteRootSecret(PathBuf),
    #[error(
        "{0} has more than {1} entries, so it cannot be checked for programs before write is \
         granted — grant a narrower folder"
    )]
    WriteRootTooLarge(PathBuf, usize),
    #[error("{0} could not be inspected before granting write: {1}")]
    WriteRootUnscannable(PathBuf, String),
    #[error("this session already holds the maximum of {0} folder grants")]
    CapReached(usize),
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
            session_path_grants: Arc::new(RwLock::new(Vec::new())),
            fs_confinement_backend: None,
            deny_cache: Arc::new(RwLock::new(None)),
            deny_walks: Arc::new(AtomicU64::new(0)),
            vcs_store_cache: Arc::new(RwLock::new(None)),
            guard_counters: Arc::new(GuardCounters::default()),
            nested_stores: Arc::new(RwLock::new(None)),
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
            session_path_grants: Arc::new(RwLock::new(Vec::new())),
            fs_confinement_backend: None,
            deny_cache: Arc::new(RwLock::new(None)),
            deny_walks: Arc::new(AtomicU64::new(0)),
            vcs_store_cache: Arc::new(RwLock::new(None)),
            guard_counters: Arc::new(GuardCounters::default()),
            nested_stores: Arc::new(RwLock::new(None)),
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
            session_path_grants: Arc::new(RwLock::new(Vec::new())),
            fs_confinement_backend: None,
            deny_cache: Arc::new(RwLock::new(None)),
            deny_walks: Arc::new(AtomicU64::new(0)),
            vcs_store_cache: Arc::new(RwLock::new(None)),
            guard_counters: Arc::new(GuardCounters::default()),
            nested_stores: Arc::new(RwLock::new(None)),
        })
    }

    pub fn trust(&self) -> WorkspaceTrust {
        self.trust
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<workspace>/.wayland-out` — the session's output root, in the one
    /// spelling a path may be HANDED OUT in.
    ///
    /// [`root`](Self::root) is `canonicalize`d at construction, which on
    /// Windows is the verbatim `\\?\C:\…` form. That is the correct spelling to
    /// ENFORCE with — every prefix match in this module compares against it —
    /// and the wrong one to hand over. `path_validation::validate_user_path`
    /// refuses the verbatim namespace outright (#644), and the legacy file
    /// tools all gate on it, so a path built by joining onto `root()` and then
    /// given to the model is a path the producing session's own `Read` refuses.
    /// MEASURED on Windows 11 26200 before this existed: `Refused to read
    /// \\?\F:\…\.wayland-out\results\toolu_01.txt: path uses a Windows device /
    /// verbatim namespace`. That is the FerroxLabs/wayland#1097 dead end again,
    /// reached by a different route — the file is written, the path is handed
    /// over, and the read is refused.
    ///
    /// `dunce::simplified` is the same reduction `wcore_agent`'s
    /// `canonical_workspace` already applies before the workspace reaches the
    /// system prompt, so the path the model is told to read back and the
    /// `Working directory:` it was given are one spelling rather than two. A
    /// pure string operation, and a plain no-op on Unix.
    #[must_use]
    pub fn session_output_root(&self) -> PathBuf {
        dunce::simplified(&self.root).join(wcore_config::config::SESSION_OUTPUT_ROOT)
    }
    /// Every root this session may WRITE.
    ///
    /// This is the SOLE producer of `SandboxManifest::fs_write_allow`
    /// (`bash.rs`), and — because [`readable_roots`](Self::readable_roots) is
    /// built on top of it — a write grant is a read grant for free, never the
    /// other way round.
    ///
    /// #1104: this filter on `grant.write` is the ONE place a standing grant
    /// turns into write authority. Every other consumer of the grant store
    /// reads the whole store and confers read only, so a write grant cannot
    /// leak out of a path that forgot to check.
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
        // Expiry is evaluated HERE rather than reaped on a timer, exactly as
        // `readable_roots` does it: this is the one place that decides what a
        // session may write, so a grant cannot outlive its deadline by racing
        // a sweep. `bash.rs` rebuilds the manifest per exec, so the next
        // command after the deadline is already narrowed.
        //
        // APPENDED, never sorted. The existing order is load-bearing: the
        // workspace root is `[0]`, the private scratch grant is the first entry
        // after it (`temp_env` points TMPDIR at whatever that is), and
        // `is_delegated_shape` compares the whole vector against
        // `[checkout, scratch]` positionally. Sorting this list to dedup it
        // silently repointed TMPDIR at `~/.cargo/registry` on a Trusted policy
        // — caught by `trusted_local_sets_cwd_and_does_not_redirect_caches`,
        // which is the only reason this comment exists.
        let now = SystemTime::now();
        for granted in self
            .session_path_grants
            .read()
            .iter()
            .filter(|grant| grant.confers(now, true))
            .map(|grant| grant.root.clone())
        {
            if !v.contains(&granted) {
                v.push(granted);
            }
        }
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
        // Expired grants are filtered here rather than reaped on a timer:
        // this is the one place that decides what a session may read, so a
        // grant cannot outlive its deadline by racing a sweep.
        let now = SystemTime::now();
        v.extend(
            self.session_path_grants
                .read()
                .iter()
                .filter(|grant| grant.confers(now, false))
                .map(|grant| grant.root.clone()),
        );
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

    /// #1104 — record that the OS sandbox selected for this session CONFINES
    /// the filesystem, naming the backend that does it.
    ///
    /// The single gate on write grants. Called at the one seam that can see the
    /// selected backend, with `SandboxBackend::confines_filesystem()` read off
    /// the SAME handle that will run this session's commands — so there is no
    /// window between reading the capability and relying on it, and no second
    /// copy of the question to drift.
    ///
    /// Deliberately NOT inferred from the platform or the trust level. Both
    /// have been wrong here before: the backend's own claim is the only thing
    /// that tracks a `WAYLAND_SANDBOX` override, a probe failure, or the
    /// fail-closed backend.
    #[must_use]
    pub fn with_filesystem_confinement(mut self, backend_name: impl Into<String>) -> Self {
        self.fs_confinement_backend = Some(backend_name.into());
        self
    }

    /// The backend confining this session's filesystem, if one does.
    #[must_use]
    pub fn filesystem_confinement_backend(&self) -> Option<&str> {
        self.fs_confinement_backend.as_deref()
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
        // #383 — WHICH resolver, named here for the same reason #356 named it
        // on the two predicates next door: this file held two with different
        // escape properties and this one kept the weaker of them.
        // `canon_for_scope` resolves only the IMMEDIATE parent and returns the
        // RAW path when the leaf cannot be canonicalized, so a DANGLING symlink
        // (`notes.txt` -> a `.env` that does not exist yet) was judged where the
        // LINK sits rather than where the write would land. That is exactly the
        // shape of a Full-posture session CREATING the project's secret, and
        // exactly what the doc comment above already claimed was closed.
        // `canon_existing_ancestor` hops the link by hand (`resolve_prefix`), so
        // the claim is now true as written. Graded by
        // `tests::a_dangling_symlink_to_a_not_yet_existing_project_secret_is_refused`.
        self.is_project_secret_resolved(&self.resolve(path))
    }

    /// [`is_project_secret`](Self::is_project_secret) on an ALREADY-RESOLVED
    /// path.
    ///
    /// Split out so [`denies_read_content`](Self::denies_read_content) can pay
    /// for the resolution once and ask both halves of the `SecretDenyFs` guard
    /// against the same answer. Two independent resolutions of one path is not
    /// merely wasted work (FerroxLabs/wayland-core#376): it is two chances for
    /// the guard's halves to disagree about where the operation lands.
    fn is_project_secret_resolved(&self, canon: &Path) -> bool {
        is_secret_path_static(canon) && canon.starts_with(&self.root)
    }

    /// FerroxLabs/wayland-core#244 + #322: true when `path` is inside a VCS
    /// CONTENT store — `.git/objects`, `.git/modules`, `.git/lfs`, `.hg/store`,
    /// `.svn/pristine`, `.bzr/repository`.
    ///
    /// The IN-PROCESS sibling of the OS-sandbox deny [`vcs_content_stores`]
    /// builds. Those two layers had drifted: `Bash` could not `git show
    /// HEAD:.env`, but the file tools read `.git/objects/ab/cdef...` straight
    /// off disk, because the only `SecretDenyFs` predicate
    /// ([`is_project_secret`](Self::is_project_secret)) matches secret NAMES and
    /// an object file is named after its hash. The blobs are zlib-compressed, so
    /// #244 is a gap rather than a plaintext leak — but a gap between two layers
    /// that are supposed to agree is how a boundary stops being one.
    ///
    /// Two arms, cheap one first:
    ///
    /// 1. **Lexical, any depth, under this workspace root** — the #322 half.
    ///    Costs nothing beyond the canonicalization the caller pays anyway, and
    ///    covers the nested/vendored store that root-relative discovery never
    ///    sees.
    /// 2. **The stores this root's own `.git` NAMES** — a gitfile's gitdir and
    ///    commondir (#242), an `objects/info/alternates` borrow, and a `.git`
    ///    SYMLINK whose target sits outside the root. None of those resolve to a
    ///    path whose parent component is still `.git`, so arm 1 cannot see them.
    ///    Reached only when arm 1 misses.
    ///
    /// NOT routed through any sandbox-backend capability, for the reason
    /// `crates/wcore-tools/tests/vfs_secret_deny_backend_independent.rs` pins:
    /// this refusal is enforced by THIS process.
    pub fn is_vcs_content_store(&self, path: &Path) -> bool {
        // #383 c3 — the SAME resolver
        // [`is_project_secret`](Self::is_project_secret) uses, for the same
        // reason and with the same escape closed. `SecretDenyFs::guard` asks
        // both predicates about one path; resolving that path two different
        // ways is how one of them ends up refusing a write the other admits.
        // Graded by `tests::a_dangling_symlink_into_a_vcs_content_store_is_refused`.
        self.is_vcs_content_store_resolved(&self.resolve(path))
    }

    /// [`is_vcs_content_store`](Self::is_vcs_content_store) on an
    /// ALREADY-RESOLVED path. See
    /// [`is_project_secret_resolved`](Self::is_project_secret_resolved) for why
    /// the split exists.
    fn is_vcs_content_store_resolved(&self, canon: &Path) -> bool {
        // Arm 1 — lexical, zero syscalls.
        if canon.starts_with(&self.root) && inside_vcs_store(canon) {
            return true;
        }
        // Arm 4 — the nested discovery. Zero syscalls once warm, so it is asked
        // before the two arms that cost probes.
        if self.nested_stores_cover(canon) {
            return true;
        }
        // Arm 3 — repository shape, path-local and always fresh. Zero syscalls
        // for a path with no store-leaf component, which is every ordinary
        // path.
        if self.encloses_repository_store(canon) {
            return true;
        }
        // Arm 2 — the stores this root's own `.git` names.
        if self
            .vcs_stores_memoized()
            .iter()
            .any(|store| canon.starts_with(store))
        {
            return true;
        }
        // FerroxLabs/wayland-core#406 c1 — every arm has missed and this call
        // is about to ADMIT. Only now is arm 4's memo worth a freshness check:
        // a store the walk FOUND was already refused above without a probe, so
        // the sole answer staleness can corrupt is this one. Costs nothing at
        // all when the walk found no nested checkout to declare anything.
        if !self.nested_declarations_moved() {
            return false;
        }
        self.nested_store_walk(canon)
    }

    /// Arm 3 — **is some ancestor of `canon` the object database of a
    /// repository that is actually there?**
    ///
    /// FerroxLabs/wayland-core#396. Arms 1 and 2 both decide from a NAME: arm 1
    /// from the query path's spelling, arm 2 from what `<root>/.git` names. A
    /// BARE repository vendored under the root (`git clone --bare|--mirror`, a
    /// submodule object cache, a vendored mirror) has no control directory at
    /// all — `objects/`, `HEAD` and `refs/` sit at its top level — so neither
    /// name-based arm can see it, and `<root>/vendor/pkg.git/objects/ab/cd`
    /// was read straight back through the VFS.
    ///
    /// This arm asks the filesystem instead, about the ancestors of the path in
    /// hand and nothing else:
    ///
    /// * cost is O(depth), never O(workspace) — the regression
    ///   FerroxLabs/wayland-core#398 records came from gating a whole-tree walk
    ///   on a path SPELLING, and there is no whole-tree walk here to gate;
    /// * an ordinary path pays ZERO probes, because no ancestor of
    ///   `src/deep/deeper/main.rs` carries a store leaf name;
    /// * nothing is memoised, so a repository that comes into being mid-session
    ///   is refused on the very next guard. `WorkspacePolicy` is built once at
    ///   bootstrap and `Arc`-cloned into `SecretDenyFs` for the whole session,
    ///   so "the state at construction" is NOT a safe answer to give for the
    ///   rest of that session.
    ///
    /// The confirmation is deliberately a REPOSITORY test and not a leaf-name
    /// test: `objects/`, `modules/`, `store/` and `lfs/` are ordinary project
    /// directory names (Terraform modules, a Redux store, an asset pipeline),
    /// and refusing them on the name alone would be a wrong refusal on real
    /// user data. Graded against that negative control by
    /// `tests::an_ordinary_directory_named_objects_is_not_a_repository`.
    fn encloses_repository_store(&self, canon: &Path) -> bool {
        if !canon.starts_with(&self.root) {
            return false;
        }
        for ancestor in canon.ancestors() {
            if ancestor == self.root {
                break;
            }
            let Some(name) = ancestor.file_name() else {
                break;
            };
            if !is_vcs_store_leaf_name(name) {
                continue;
            }
            let Some(parent) = ancestor.parent() else {
                continue;
            };
            if self.is_repository_dir(parent) {
                return true;
            }
        }
        false
    }

    /// True when `dir` is the top level of a git repository — a BARE repository
    /// or the gitdir a `.git` file points at, which have the same layout.
    ///
    /// `HEAD` plus one of `refs`/`config`, which is what `git` itself requires
    /// of a directory before it will treat it as a repository, and two `stat`s
    /// at most. A single marker is not enough: `HEAD` alone is a plausible
    /// name for an ordinary data file.
    ///
    /// Git-shaped only, and that is a scope decision rather than an oversight.
    /// `.hg`/`.svn`/`.bzr` keep their stores INSIDE the control directory in
    /// every layout they support, so arm 1 already sees them at any depth;
    /// git is the only one of the four with a documented bare layout that
    /// hoists a content store to a directory's top level.
    fn is_repository_dir(&self, dir: &Path) -> bool {
        self.probe_exists(&dir.join("HEAD"))
            && (self.probe_exists(&dir.join("refs")) || self.probe_exists(&dir.join("config")))
    }

    /// One counted filesystem existence probe. Counted through the same
    /// [`GuardCounters`] the cost tests read, so an arm that starts probing
    /// cannot do it invisibly.
    fn probe_exists(&self, path: &Path) -> bool {
        self.guard_counters.probes.fetch_add(1, Ordering::Relaxed);
        std::fs::symlink_metadata(path).is_ok()
    }

    /// Arm 4 — every content store DECLARED by a control directory nested under
    /// the root, discovered once and cached for the life of the policy.
    ///
    /// FerroxLabs/wayland-core#390 c1/c2 and #394 c1. Arm 2 resolves what
    /// `<root>/.git` names and nothing else, so a VENDORED checkout fell
    /// between the arms: `<root>/vendor/pkg/.git` is the file
    /// `gitdir: ../pkg-git`, its store at `<root>/vendor/pkg-git/objects` is
    /// not lexically a `(control, store)` pair, and the root's `.git` never
    /// mentions it. The same hole admits an `objects/info/alternates` borrow
    /// declared by a nested checkout — and that one is admitted REGARDLESS of
    /// how the borrow target is spelled, because an alternates entry names an
    /// arbitrary directory.
    ///
    /// The store set is resolved EAGERLY at discovery time and tested by
    /// prefix, so a store's own directory name never enters the decision. That
    /// is what makes #394's class (a borrow target called `odb`, a `.git/objects`
    /// symlink pointing at one) impossible rather than enumerated.
    ///
    /// **Once, and never revalidated.** Stated plainly because it is the cost
    /// this design pays: a store DECLARED after this walk, at a path arms 1 and
    /// 3 cannot see path-locally, stays admitted for the rest of the session
    /// (FerroxLabs/wayland-core#406). The alternative — stamping every
    /// descended directory and re-`stat`ing them per guard — is the measured
    /// regression #398 was filed about, and it scales with the tree.
    fn nested_stores_cover(&self, canon: &Path) -> bool {
        // Tested UNDER the read lock rather than through a clone: this runs on
        // every guard, and handing the caller an owned `Vec<PathBuf>` would put
        // one allocation per store on the hot path that #376 exists to keep
        // flat.
        if let Some(cache) = self.nested_stores.read().as_ref() {
            return cache.stores.iter().any(|store| canon.starts_with(store));
        }
        self.nested_store_walk(canon)
    }

    /// Run arm 4's walk, replace the memo, and report whether the set it
    /// produced covers `canon`. Counted.
    fn nested_store_walk(&self, canon: &Path) -> bool {
        let scan = discover_nested_content_stores(&self.root);
        self.guard_counters
            .nested_walks
            .fetch_add(1, Ordering::Relaxed);
        self.guard_counters
            .probes
            .fetch_add(scan.probes, Ordering::Relaxed);
        let covered = scan.stores.iter().any(|store| canon.starts_with(store));
        *self.nested_stores.write() = Some(NestedStoreCache {
            stamped_at: scan.stamped_at,
            witnesses: scan.witnesses,
            stores: scan.stores,
        });
        covered
    }

    /// FerroxLabs/wayland-core#406 c1 — has any DECLARATION SITE arm 4 read
    /// changed since the walk read it?
    ///
    /// Asked only on the branch that is about to ADMIT, because that is the
    /// only branch whose answer a stale memo can get wrong: a store the walk
    /// FOUND is refused from the set with no probe at all, and #406's own body
    /// locates the tension exactly here — any per-call freshness check for a
    /// whole-tree fact costs at least one probe, and the ordinary-path guard is
    /// pinned at three.
    ///
    /// The cost is therefore stated precisely: **zero probes when the walk
    /// found no nested checkout** (the witness set is empty, so an ordinary
    /// workspace's guard is byte-for-byte what it was), and **one probe per
    /// declaration site otherwise** — O(nested checkouts), independent of the
    /// directory count, which is what keeps #398 c1's slope at zero.
    ///
    /// `<root>/.git`'s own declarations are deliberately NOT in this set: arm 2
    /// (`vcs_stores_memoized`) already stamps and revalidates that gitfile, its
    /// `commondir` and its `alternates` on every guard, and they are three of
    /// the three probes #398 c2 pins. Witnessing them twice would move that
    /// number for no extra denial.
    ///
    /// **What this does NOT see, stated rather than implied.** A control
    /// directory that did not exist when the walk ran and that declares a
    /// borrow at a target which is neither repository-shaped (arm 3) nor
    /// lexically a store (arm 1) has no declaration site in this set to move.
    /// Seeing that needs a witness per DESCENDED DIRECTORY, which is #398's
    /// regression. Pinned as a measurement by
    /// `vfs_nested_store_deny.rs::a_borrow_declared_by_a_control_dir_created_after_the_walk_is_still_admitted`.
    fn nested_declarations_moved(&self) -> bool {
        let guard = self.nested_stores.read();
        let Some(cache) = guard.as_ref() else {
            return false;
        };
        for (path, seen) in &cache.witnesses {
            self.guard_counters.probes.fetch_add(1, Ordering::Relaxed);
            let now = std::fs::symlink_metadata(path)
                .and_then(|meta| meta.modified())
                .ok();
            if now != *seen {
                return true;
            }
            // Same same-tick hazard `vcs_store_cache_hit` documents (#1145): a
            // write inside the walk's own window cannot be witnessed by an
            // mtime at all, so an unsettled stamp is treated as moved.
            if let Some(now) = now
                && !stamp_is_settled(now, cache.stamped_at)
            {
                return true;
            }
        }
        false
    }

    /// The arm-2 store list, memoised behind a witness stamp
    /// (FerroxLabs/wayland-core#376 c2).
    ///
    /// MEASURED before the memo, by differential `strace` over the release
    /// example `secret_deny_cost`: arm 2 cost **12 filesystem syscalls on every
    /// ordinary-path operation** (6 `exists` for the root-relative store leaves,
    /// 1 `metadata` for the gitfile probe, 1 `openat` for the `alternates` read,
    /// and 4 `readlink` canonicalizing the one leaf that exists), out of 18 for
    /// the whole guard and 32 for a complete `SandboxedFs`+`SecretDenyFs`
    /// `exists()`. `SecretDenyFs` is installed unconditionally for every
    /// sub-agent (`spawner.rs`) and every channel/remote session
    /// (`channel_tools.rs`), and sub-agents are read-heavy.
    ///
    /// Revalidation is NOT a weaker answer than a rescan:
    ///
    /// * A witness that was PRESENT must still report the exact mtime the scan
    ///   saw, and that mtime must lag the scan's own start instant by more than
    ///   one filesystem tick — the #1145 granularity rule, shared verbatim with
    ///   [`deny_cache_hit`](Self::deny_cache_hit) through
    ///   [`stamp_is_settled`].
    /// * A witness that was ABSENT must still be absent, checked NOW. A store
    ///   that exists now has a witness that exists now, so it cannot be missed;
    ///   a store that appeared and vanished between the scan and now is not
    ///   there to be read either.
    ///
    /// Any unreadable witness, any difference, any unsettled mtime is a miss,
    /// and a miss rescans. The cache is never trusted through a hole in its
    /// stamp.
    fn vcs_stores_memoized(&self) -> Vec<PathBuf> {
        if let Some(hit) = self.vcs_store_cache_hit() {
            return hit;
        }
        let scan = scan_vcs_content_stores(&self.root);
        self.guard_counters.scans.fetch_add(1, Ordering::Relaxed);
        self.guard_counters
            .probes
            .fetch_add(scan.probes, Ordering::Relaxed);
        *self.vcs_store_cache.write() = Some(VcsStoreCache {
            stamped_at: scan.stamped_at,
            witnesses: scan.witnesses,
            stores: scan.stores.clone(),
        });
        scan.stores
    }

    /// The memoised store list, but ONLY if a fresh scan would produce the same
    /// one. See [`vcs_stores_memoized`](Self::vcs_stores_memoized).
    fn vcs_store_cache_hit(&self) -> Option<Vec<PathBuf>> {
        let guard = self.vcs_store_cache.read();
        let cache = guard.as_ref()?;
        for (path, seen) in &cache.witnesses {
            self.guard_counters.probes.fetch_add(1, Ordering::Relaxed);
            let now = std::fs::symlink_metadata(path)
                .and_then(|meta| meta.modified())
                .ok();
            if now != *seen {
                return None;
            }
            // An absent witness is decided at THIS instant, so there is no
            // window for it to be unsettled in. A present one carries the same
            // same-tick hazard `deny_cache_hit` documents.
            if let Some(now) = now
                && !stamp_is_settled(now, cache.stamped_at)
            {
                return None;
            }
        }
        Some(cache.stores.clone())
    }

    /// THE read-content refusal: true when the in-process file tools must not
    /// hand `path`'s bytes to the model.
    ///
    /// The whole of what [`crate::vfs::SecretDenyFs`] asks, in one place, so
    /// every OTHER call site that has to agree with that boundary can ask the
    /// identical question instead of assembling its own conjunction. `GrepTool`
    /// is the call site that forced this (FerroxLabs/wayland-core#375): it
    /// spawns `rg` OUTSIDE the VFS and outside the OS sandbox, so the store
    /// deny reached it through neither layer, and a second name list in
    /// `grep_policy.rs` would have been one more thing to drift.
    ///
    /// Resolves the path ONCE and hands the result to both halves — see
    /// [`is_project_secret_resolved`](Self::is_project_secret_resolved).
    pub fn denies_read_content(&self, path: &Path) -> bool {
        let canon = self.resolve(path);
        self.is_project_secret_resolved(&canon) || self.is_vcs_content_store_resolved(&canon)
    }

    /// [`canon_existing_ancestor`], counted. See [`GuardCounters`].
    fn resolve(&self, path: &Path) -> PathBuf {
        // #356 c4 — resolver: `canon_existing_ancestor`, because both predicates this feeds
        // (`is_project_secret_resolved`, `is_vcs_content_store_resolved`) are
        // security REFUSALS, and a refusal must judge where a path lands, not
        // where its spelling sits. #383 moved them here from the weak one.
        self.guard_counters.resolves.fetch_add(1, Ordering::Relaxed);
        canon_existing_ancestor(path)
    }

    /// #376 c3 — (path resolutions, store scans, filesystem probes) charged by
    /// the `SecretDenyFs` guard predicates since this policy was constructed.
    ///
    /// The instrument the per-operation cost is pinned with. Counted, not
    /// timed: on a loaded host a wall clock cannot tell a skipped scan from a
    /// fast one, and the regression this guards against (a rebuild returning to
    /// the common path) is invisible in a timing that is dominated by
    /// scheduling noise.
    /// FerroxLabs/wayland-core#398 — how many times arm 4's nested-store
    /// discovery walk has run for this policy. One, for the life of the
    /// policy; the assertion that it stays at one is what makes arm 4's
    /// per-guard cost independent of the workspace size a fact rather than a
    /// claim.
    #[doc(hidden)]
    pub fn nested_walk_count(&self) -> u64 {
        self.guard_counters.nested_walks.load(Ordering::Relaxed)
    }

    #[doc(hidden)]
    pub fn guard_cost(&self) -> (u64, u64, u64) {
        (
            self.guard_counters.resolves.load(Ordering::Relaxed),
            self.guard_counters.scans.load(Ordering::Relaxed),
            self.guard_counters.probes.load(Ordering::Relaxed),
        )
    }

    /// True when `path` names this workspace's own REPOSITORY-CONTROL surface
    /// ([`REPO_CONTROL_DIRS`]) — the directories whose contents are executed or
    /// obeyed rather than merely read back.
    ///
    /// The predicate for a WRITE deny, never a read deny. Reading `.git/HEAD`
    /// and loading `.wayland-core/skills/**` are ordinary session work; what
    /// must not happen is the model AUTHORING those bytes. A `Write` of
    /// `.git/hooks/pre-commit` is arbitrary code execution on the operator's
    /// next commit, and a `Write` of `.wayland-core/skills/x/SKILL.md` is
    /// arbitrary instruction injection into the next session — the very surface
    /// [`wcore_config::workspace_trust::fingerprint_workspace`] hashes in order
    /// to bind a trust grant to it. A tool that can rewrite that surface can
    /// invalidate the grant's meaning without the operator ever seeing a prompt.
    ///
    /// Deliberately WORKSPACE-SCOPED and canonicalize-first, exactly like
    /// [`is_project_secret`](Self::is_project_secret): the `<root>/.git` of THIS
    /// session is protected, a `.git` elsewhere on the host is not this policy's
    /// business, and a benign-named symlink into the control surface resolves
    /// before the prefix match so it cannot be used to smuggle a write through.
    pub fn is_repo_control_path(&self, path: &Path) -> bool {
        // `canon_existing_ancestor`, not `canon_for_scope`: the root is
        // canonicalized at construction, so a candidate that resolves to
        // something shallower never matches it. `canon_for_scope` resolves only
        // the IMMEDIATE parent and returns the RAW path when that parent is
        // missing — which is exactly the shape of a NEW control file
        // (`.wayland-core/skills/<new>/SKILL.md`, `.git/<new>/hook`), and
        // exactly the write this guard exists to refuse. Measured on this
        // tree before that change: with the workspace addressed through a
        // symlink, `Write` of `<link>/.git/hooks/pre-commit` (parent not yet
        // created) reported `Created`, while the same write addressed through
        // the real root was refused. See
        // `crates/wcore-tools/tests/repo_control_symlink.rs`.
        //
        // #356 c4 — resolver: `canon_existing_ancestor`.
        // It was `canon_deep` until this line, and the doc comment above
        // claims a benign-named symlink "resolves before the prefix match".
        // That was only true of a link whose target already exists —
        // `std::fs::canonicalize` fails on a DANGLING one, and the walk-up form
        // then judged where the LINK sits. `canon_existing_ancestor` hops the
        // link by hand (`resolve_prefix`), so the claim above is now true as
        // written. Graded by
        // `tests::a_dangling_symlink_into_the_skill_load_path_is_refused`.
        let canon = canon_existing_ancestor(path);
        REPO_CONTROL_DIRS
            .iter()
            .any(|dir| canon.starts_with(self.root.join(dir)))
    }

    /// True when `path` names a directory skills are LOADED from — this
    /// workspace's (or any ancestor's) `.wayland-core/skills` and
    /// `.wayland-core/commands`, or the user-level `<config_dir>/skills` and
    /// `<config_dir>/commands`.
    ///
    /// FerroxLabs/wayland#1096, suggested direction 2. A load path is not an
    /// output path, and the everyday failure is not malice: a skill produces a
    /// report and puts it next to its own `SKILL.md`, which lives in the global
    /// config dir, OUTSIDE the session workspace. The file is then unreachable
    /// to the session that made it — the dead end the 2026-08-19 UAT hit, and
    /// reproduced through the live binary in
    /// `wcore-cli/tests/skill_source_write_live.rs`.
    ///
    /// Strictly WIDER than [`is_repo_control_path`](Self::is_repo_control_path),
    /// and that is the whole point of it being separate. Repo-control is
    /// workspace-scoped (`<root>/.wayland-core`), because a `.git` elsewhere on
    /// the host is not this policy's business. A skill LOAD path is: the
    /// user-level directory is read into every future session on the machine no
    /// matter where the workspace sits, and `project_skills_dirs()` walks
    /// ANCESTORS of the cwd, so a `.wayland-core/skills` above the workspace
    /// root is loaded too. Both are refused here; neither is reachable from the
    /// workspace-scoped predicate.
    ///
    /// Not a read deny. The loader reads these paths on every boot and the model
    /// may inspect a skill it is about to run; only AUTHORING them is refused.
    /// Also deliberately narrow inside the config dir — session state, memory
    /// and plugin data live there too and stay writable.
    ///
    /// Refuses the model's TOOLS, not the engine. The auto-skill drafter
    /// (`wcore_agent::auto_skill::drafter`) and the `skills` CLI verbs write
    /// their `SKILL.md` files through `wcore_config::atomic_write` / `std::fs`,
    /// never the tool VFS, so skill installation and drafting are unaffected.
    pub fn is_skill_source_path(&self, path: &Path) -> bool {
        // #356 c4 — resolver: `canon_existing_ancestor`: WHICH resolver, stated
        // here rather than only at its definition, because this file used to hold two with different escape
        // properties seventy lines apart and a reader here could not see that a
        // choice had been made. `canon_existing_ancestor` walks DOWN and
        // re-resolves after every component; the walk-UP-and-append-verbatim
        // form this call site used until #356 (`canon_deep`) could not see a
        // path that reaches a load path through a DANGLING symlink, because
        // `std::fs::canonicalize` fails on one and the component was then kept
        // verbatim. Both escapes #1097 was written for are graded on THIS
        // predicate by `tests::a_dangling_symlink_into_the_skill_load_path_is_refused`
        // and `tests::a_parent_dir_after_a_missing_component_still_reaches_the_skill_load_path`,
        // and end to end through the real `Write` tool by
        // `wcore-agent/tests/skill_source_write_refusal.rs`.
        let canon = canon_existing_ancestor(path);
        if under_project_load_path(&canon) {
            return true;
        }
        wcore_config::config::user_skill_source_dirs()
            .iter()
            .any(|dir| canon.starts_with(canon_existing_ancestor(dir)))
    }

    /// Refuse a write target this session could never read back
    /// (FerroxLabs/wayland#1097).
    ///
    /// The invariant: a path we let an agent WRITE must sit under a root
    /// [`readable_roots`](Self::readable_roots) also covers. Where it does not,
    /// the work still succeeds and the delivery fails — the agent finishes
    /// holding a path it just created and cannot open. Refusing at write time
    /// costs the same information one step earlier, with the reason named.
    ///
    /// Canonicalize-first, exactly like
    /// [`is_repo_control_path`](Self::is_repo_control_path): the longest
    /// EXISTING ancestor is resolved (so a target whose directories do not
    /// exist yet is still judged on where it would really land), which also
    /// means a `..` segment and a symlinked parent are resolved before the
    /// prefix match rather than after it.
    ///
    /// SCOPE — this is the OS-sandbox answer, which is the WIDER of the two
    /// answers this codebase has to "can the session read it". `Bash` reads
    /// through the OS sandbox, whose allow-list IS `readable_roots()`; the file
    /// tools (`Read`/`Grep`/`Glob`) read through `ctx.vfs`, whose jail is
    /// rooted at the workspace plus the standing session read grants and does
    /// NOT include `readable_extra` (toolchain dirs) or the writable scratch
    /// tree. So passing this check is NECESSARY for a readable-back write and
    /// is not by itself SUFFICIENT for one the `Read` tool can open: a caller
    /// that hands its path to the model should keep the target under
    /// [`root`](Self::root).
    pub fn ensure_write_target_readable(&self, path: &Path) -> Result<(), WriteTargetNotReadable> {
        // #356 c4 — resolver: `canon_existing_ancestor`, on BOTH sides of the
        // comparison. A write target whose directories do not exist yet must
        // still be judged on where it would land, and a readable root reached
        // through a symlink must be compared as its target; resolving only one
        // side would make the prefix match answer on spelling.
        let resolved = canon_existing_ancestor(path);
        let covered = self
            .readable_roots()
            .iter()
            .any(|root| resolved.starts_with(canon_existing_ancestor(root)));
        if covered {
            Ok(())
        } else {
            Err(WriteTargetNotReadable {
                path: path.to_path_buf(),
            })
        }
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
        if local_operator_shell_principal(channel_posture_present, managed_execution_floor) {
            self.with_local_operator_principal()
        } else {
            self
        }
    }

    /// #1118 — a DELEGATED CHILD's shell principal is its parent's, and can
    /// never be wider.
    ///
    /// The sub-agent seam has no channel scope of its own to inspect: it is
    /// reached only from a parent session that already made this decision at a
    /// seam that could see one ([`with_shell_principal`](Self::with_shell_principal)).
    /// Before this existed the spawner made no decision at all, so every child
    /// fell to the fail-safe `false` while its parent — same operator, same
    /// machine, same workspace — held `true`, and `bash.rs` refused the child's
    /// shell on any backend that cannot enforce OS read-deny (the Windows
    /// session default) while running the parent's.
    ///
    /// This is principal DERIVATION, not a profile relaxation:
    /// `secret_read_deny_required` is untouched, the deny LIST is still computed
    /// and still handed to the backend, and a backend that can enforce it (Linux
    /// `bwrap`, macOS `sandbox_exec`, `docker`) still enforces every path in it
    /// — including the delegating parent's own workspace. What changes is
    /// confined to backends that were going to enforce nothing either way, where
    /// the refusal removed the child's shell without removing anything a parent
    /// shell on the same machine could not already reach.
    #[must_use]
    pub fn with_inherited_shell_principal(self, parent_is_local_operator: bool) -> Self {
        if parent_is_local_operator {
            self.with_local_operator_principal()
        } else {
            self
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
    /// Also denies the VCS CONTENT stores ([`vcs_content_stores`]) so a committed
    /// secret cannot be reconstructed from `.git/objects` via `Bash("git show
    /// HEAD:.env")` and friends — the sibling of the typed-GitTool drop (MF1).
    pub fn secret_deny_paths_dynamic(&self) -> Vec<PathBuf> {
        self.secret_deny_paths_stamped().0
    }

    /// #1111 — how many times this policy has recomputed the deny set from the
    /// filesystem. The injected counter the repeated-walk acceptance is graded
    /// with; a wall clock cannot tell a skipped walk from a fast one.
    #[doc(hidden)]
    pub fn secret_deny_walk_count(&self) -> u64 {
        self.deny_walks.load(Ordering::Relaxed)
    }

    /// [`secret_deny_paths_dynamic`](Self::secret_deny_paths_dynamic) plus the
    /// stamp a later call needs to decide whether that answer is still current.
    ///
    /// The stamp is every DIRECTORY the walk descended into with its mtime, and
    /// the instant the walk started. Secrecy is decided by NAME
    /// ([`secret_entry`]), never by content, so the only events that can change
    /// this answer are an entry being created, renamed, deleted or re-pointed —
    /// and every one of those updates the containing directory's mtime. A stamp
    /// over directories therefore detects every change that matters, WHOEVER
    /// made it: this process, the operator's editor, a `git checkout`, or an
    /// unrelated program. That is strictly stronger than invalidating on writes
    /// through our own VFS, which sees only the first of those.
    fn secret_deny_paths_stamped(&self) -> (Vec<PathBuf>, Vec<(PathBuf, SystemTime)>, SystemTime) {
        // Taken BEFORE the walk: anything modified from here on is inside the
        // walk's own window and must not be trusted by a later revalidation.
        let stamped_at = SystemTime::now();
        self.deny_walks.fetch_add(1, Ordering::Relaxed);
        let mut dirs: Vec<(PathBuf, SystemTime)> = Vec::new();
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
        let mut out = compute_secret_deny(base_trust, &self.root, &readable_canon, &mut dirs);
        if self.secret_read_deny_required {
            out.extend(project_committed_secrets(
                &self.root,
                &readable_canon,
                &mut dirs,
            ));
            out.extend(vcs_content_stores(&self.root));
            // A granted folder is a mounted root the child can reach, so it
            // needs the same secret walk the workspace gets. Without this the
            // in-process file tools would refuse `<granted>/id_rsa` while
            // `Bash cat` read it happily, and the two layers would disagree
            // about what is readable — which is how a boundary quietly stops
            // being one.
            // Each granted root is walked ONCE, and only when its subtree is
            // not already covered by a walk that has run. `grant_capacity`
            // refuses a grant UNDER a live grant but never compares one against
            // the workspace root, so granting the workspace itself (a host's
            // "always allow this folder" on the project directory) bought a
            // second full walk of the same tree for a byte-identical answer —
            // measured at 2.01x in `tests/walk_parallel_identity_test.rs`.
            //
            // Skipping a covered grant cannot narrow the deny set. A secret it
            // would emit has a canonical path starting with the covering walk
            // root, and the covering walk's own scope admits exactly that:
            // `readable_canon` for the workspace root (which `writable_roots`
            // puts in it), and the grant itself for a granted root. Coverage is
            // also checked for REACHABILITY, not just prefix — see
            // `walk_root_is_covered`.
            let mut walked = vec![canon(self.root.clone())];
            let mut granted_roots = self.session_path_grant_roots();
            // An ancestor sorts before its descendants, so one greedy pass
            // retains the covering root and drops what it covers whatever order
            // the grants were minted in.
            granted_roots.sort();
            for granted in granted_roots {
                if walked
                    .iter()
                    .any(|covering| walk_root_is_covered(covering, &granted))
                {
                    continue;
                }
                let scope = vec![granted.clone()];
                out.extend(project_committed_secrets(&granted, &scope, &mut dirs));
                walked.push(granted);
            }
        }
        out.extend(self.authority_read_deny.iter().cloned());
        out.sort();
        out.dedup();
        (out, dirs, stamped_at)
    }

    /// #1111 — everything about the POLICY that changes the deny answer. A
    /// difference here misses outright, before a single directory is stat'ed.
    ///
    /// `readable_roots` already folds in the network posture, the developer
    /// capability roots and the LIVE session read grants (expiry included),
    /// which is the whole of what moves the walk's scope.
    fn deny_cache_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.root.hash(&mut hasher);
        matches!(self.trust, WorkspaceTrust::Contained).hash(&mut hasher);
        self.secret_read_deny_required.hash(&mut hasher);
        self.authority_read_deny.hash(&mut hasher);
        self.readable_roots().hash(&mut hasher);
        self.session_path_grant_roots().hash(&mut hasher);
        hasher.finish()
    }

    /// #1111 — the memoised answer, but ONLY if a fresh walk would produce the
    /// same one.
    ///
    /// Every stamped directory must still report the exact mtime the walk saw.
    /// Any failure to read one, any difference, any sentinel and any mtime at or
    /// after the walk's own start instant is a miss — the cache is never trusted
    /// through a hole in its stamp.
    fn deny_cache_hit(&self, key: u64) -> Option<Vec<PathBuf>> {
        let guard = self.deny_cache.read();
        let cache = guard.as_ref()?;
        if cache.key != key {
            return None;
        }
        for (dir, seen) in &cache.dirs {
            // The sentinel `dir_stamp` records for a directory whose mtime the
            // platform would not report. It can never match, so an unstampable
            // directory disables the memo instead of punching a hole in it.
            if *seen == SystemTime::UNIX_EPOCH {
                return None;
            }
            let now = std::fs::symlink_metadata(dir).ok()?.modified().ok()?;
            // The timestamp-granularity guard (#1145): a write that landed in
            // the same COARSE filesystem tick as the walk leaves an mtime the
            // equality test above cannot distinguish from the one recorded, so
            // an mtime is evidence of quiescence only once it is further behind
            // the walk's own start instant than one tick can account for.
            // Comparing against `stamped_at` alone is not enough - `stamped_at`
            // is fine-grained, so a same-tick mtime is strictly LESS than it and
            // the old `now >= stamped_at` test never fired.
            // Take the granularity from the stamp actually in hand rather than
            // assuming the build host's: a filesystem that resolves only whole
            // seconds reports a zero nanosecond part for every mtime it stamps.
            // A sub-second stamp that happens to land exactly on a second
            // boundary is misread here, and falls in the conservative
            // direction - a walk that was not strictly needed, never a hit that
            // was not earned.
            if now != *seen || !stamp_is_settled(now, cache.stamped_at) {
                return None;
            }
        }
        Some(cache.paths.clone())
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
        // #1111: this is the EXEC path — one call per `Bash` execution, for the
        // life of the session, over a tree that is usually identical to the one
        // the last execution walked. `secret_deny_paths_dynamic` itself stays
        // uncached, so the determinism and identity contracts that grade the
        // walk keep grading a real walk every time they ask for one.
        let key = self.deny_cache_key();
        if let Some(hit) = self.deny_cache_hit(key) {
            return hit;
        }
        let (paths, dirs, stamped_at) = self.secret_deny_paths_stamped();
        // An empty stamp means no walk happened (a `Trusted` policy with no
        // project-secret denial), so there is nothing to memoise and nothing
        // that could go stale; storing it would be a cache that can only ever
        // be wrong.
        *self.deny_cache.write() =
            (!dirs.is_empty() && dirs.len() <= DENY_CACHE_MAX_DIRS).then(|| DenyCache {
                key,
                stamped_at,
                dirs,
                paths: paths.clone(),
            });
        paths
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
            let mut grants = self.session_path_grants.write();
            for root in &roots {
                if !grants.iter().any(|existing| existing.root == *root) {
                    grants.push(SessionPathGrant {
                        root: root.clone(),
                        id: None,
                        expires_at: None,
                        // A developer capability is a toolchain the session may
                        // RUN and read; it never asked to rewrite the toolchain.
                        write: false,
                    });
                }
            }
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

    /// Standing grants held by this session that have not expired, whatever
    /// access they confer. Read-scoped by construction: every grant confers
    /// read, so this is the right list for a caller asking "what extra roots
    /// can this session reach" (the secret walk, the deny-cache key).
    pub fn session_path_grant_roots(&self) -> Vec<PathBuf> {
        let now = SystemTime::now();
        self.session_path_grants
            .read()
            .iter()
            .filter(|grant| grant.is_live(now))
            .map(|grant| grant.root.clone())
            .collect()
    }

    /// Withdraw the grant carrying `grant_id`. Returns the root that was
    /// withdrawn, or `None` if no such grant is held — an unknown id is a
    /// no-op so a host can revoke idempotently (a host that crashed mid-flow
    /// should be able to clean up without having to know what landed).
    pub fn revoke_session_read_root(&self, grant_id: &str) -> Option<PathBuf> {
        let mut grants = self.session_path_grants.write();
        let index = grants
            .iter()
            .position(|grant| grant.id.as_deref() == Some(grant_id))?;
        Some(grants.remove(index).root)
    }

    /// Shared handle to the standing path grants, for the in-process file
    /// tools.
    ///
    /// `readable_roots()` already folds these in for the Bash OS sandbox, but
    /// `SandboxedFs` is constructed with a single root and would otherwise
    /// keep refusing a granted path — the user would approve the folder and
    /// `Read` would still say no. Handing out the same `Arc` (never a copy)
    /// is what makes a grant take effect on the very next call.
    #[must_use]
    pub fn session_path_grant_handle(&self) -> Arc<RwLock<Vec<SessionPathGrant>>> {
        Arc::clone(&self.session_path_grants)
    }

    /// True when the in-process file tools can already READ `path`.
    ///
    /// Deliberately mirrors [`crate::vfs::SandboxedFs::contain_read`] — the
    /// jail root plus the live standing grants — and NOT `readable_roots()`,
    /// which is the wider set the OS shell sandbox mounts. The two lists
    /// answer different questions, and the escalation prompt must be keyed to
    /// the layer that will actually do the refusing: a prompt keyed to the
    /// wider list would stay silent for a path the tool then refuses anyway.
    pub fn is_read_reachable(&self, path: &Path) -> bool {
        // #383 c3 — resolver: `canon_for_scope`, because this predicate
        // decides whether to PROMPT, never whether to permit;
        // `SandboxedFs::contain_read` does the permitting and does its own
        // dangling-link resolution. (The reason used to be stated on
        // the write-grant predicate deleted under
        // FerroxLabs/wayland-core#384 as an enforcement predicate that
        // enforced nothing.)
        let canon = canon_for_scope(path);
        canon.starts_with(&self.root) || self.is_session_read_granted(&canon)
    }

    /// True when `path` is inside a standing read grant.
    pub fn is_session_read_granted(&self, path: &Path) -> bool {
        // #383 c3 — resolver: `canon_for_scope`. Advisory mirror, exactly like
        // [`is_read_reachable`](Self::is_read_reachable).
        let canon = canon_for_scope(path);
        let now = SystemTime::now();
        self.session_path_grants
            .read()
            .iter()
            .filter(|grant| grant.is_live(now))
            .any(|grant| canon.starts_with(&grant.root))
    }

    /// Add a standing READ grant for a folder outside the workspace, minted by
    /// an approved `ApprovalScope::AlwaysPath`.
    ///
    /// This is the "always allow this folder" answer to an escalation prompt.
    /// It is deliberately the narrowest thing that resolves the dead end: the
    /// sandbox stays in place and the workspace root is unchanged; one extra
    /// root becomes readable for the rest of the process lifetime.
    ///
    /// GATE: a grant is only ever minted for a genuinely-local session
    /// (`local_operator_principal`). A path grant expands filesystem authority
    /// past the sandbox root, which is precisely what an untrusted wire peer
    /// would like to arrange, so it gets the same treatment as
    /// `SessionMode::Force` (GHSA-8r7g) — a peer may ask, only a local
    /// operator may permit. The flag is fail-safe `false` in every constructor,
    /// so a future construction path cannot acquire this by omission.
    ///
    /// WRITE is a SEPARATE, STRICTER grant (#1104), never the read grant with
    /// a flag set. `write = true` applies every refusal above and then
    /// [`check_write_grantable`](Self::check_write_grantable) on top: an OS
    /// sandbox that actually confines the filesystem, no overlap with any
    /// auto-run location, and a bounded scan that refuses a root already
    /// holding an executable, a secret, or a `.git`. A read grant NEVER
    /// implies write, and a live read grant is not cover for a write request —
    /// see [`grant_capacity`].
    pub fn grant_session_read_root(
        &self,
        root: impl AsRef<Path>,
        write: bool,
    ) -> Result<PathBuf, PathGrantError> {
        self.grant_session_read_root_full(root, write, None, None)
    }

    /// [`grant_session_read_root`](Self::grant_session_read_root) with the
    /// host-supplied revocation id and expiry carried by the `grant_path`
    /// protocol command.
    pub fn grant_session_read_root_full(
        &self,
        root: impl AsRef<Path>,
        write: bool,
        grant_id: Option<String>,
        expires_at: Option<SystemTime>,
    ) -> Result<PathBuf, PathGrantError> {
        let dir = self.grantable_read_root(root, write)?;

        let now = SystemTime::now();
        let mut grants = self.session_path_grants.write();
        // Re-checked under the WRITE lock, because the dry run above took only
        // a read lock and a concurrent grant may have landed since.
        if grant_capacity(&grants, &dir, now, write)? {
            return Ok(dir);
        }
        grants.push(SessionPathGrant {
            root: dir.clone(),
            id: grant_id,
            expires_at,
            write,
        });
        Ok(dir)
    }

    /// Dry-run half of [`grant_session_read_root_full`](Self::grant_session_read_root_full):
    /// every check, no mutation, returning the folder that WOULD be granted.
    ///
    /// This exists so the pre-flight escalation prompt (#1099) can promise
    /// something. Core only shows a host an "always allow this folder" button
    /// after this returns `Ok`, so the button cannot be one that silently
    /// fails — the alternative is a card the user answers, a grant the policy
    /// refuses, and a re-prompt on the very next sibling file.
    pub fn grantable_read_root(
        &self,
        root: impl AsRef<Path>,
        write: bool,
    ) -> Result<PathBuf, PathGrantError> {
        // #356 c4 -- resolver: `grantable_read_root_shape`, whose own answer is
        // bare `std::fs::canonicalize`. A GRANT must name a folder that is
        // there now, so refusing a path that does not exist is the point;
        // neither guard resolver would do, because both are built to answer
        // for a path that does not exist yet.
        let dir = self.grantable_read_root_shape(root, write)?;
        let now = SystemTime::now();
        let grants = self.session_path_grants.read();
        grant_capacity(&grants, &dir, now, write)?;
        Ok(dir)
    }

    /// The path-shape half: is this a folder we are willing to open at all?
    ///
    /// #1104 — the write-only checks run LAST, after the folder has been
    /// resolved and has passed every rule a read grant must pass. Ordering is
    /// load-bearing for the message the user reads: a `$HOME` grant asked for
    /// with `write` should say "grant the specific folder instead", not
    /// "that folder holds an executable", because the first is the reason and
    /// the second is an accident of which check ran first.
    fn grantable_read_root_shape(
        &self,
        root: impl AsRef<Path>,
        write: bool,
    ) -> Result<PathBuf, PathGrantError> {
        if !self.local_operator_principal {
            return Err(PathGrantError::RequiresLocalOperator);
        }

        let canon = std::fs::canonicalize(root.as_ref())?;
        // A grant names a FOLDER. If the host sent the file the user was
        // looking at, grant the directory that contains it — that is what the
        // person answering "always allow this folder" believes they said.
        let dir = if canon.is_dir() {
            canon
        } else {
            canon
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| PathGrantError::NoParent(canon.clone()))?
        };

        if dir.parent().is_none() {
            return Err(PathGrantError::FilesystemRoot);
        }
        if let Some(home) = dirs::home_dir() {
            // #383 c3 — resolver: `canon_for_scope`. `$HOME` always exists, so the
            // two resolvers cannot disagree about it; the cheap one is chosen
            // because there is nothing here for the deep one to resolve.
            let home = canon_for_scope(&home);
            // `$HOME` itself, and anything containing it, reach everything the
            // sandbox exists to stand between the agent and.
            if home.starts_with(&dir) {
                return Err(PathGrantError::TooBroad(dir));
            }
        }
        if path_is_in_credential_store(&dir) || credential_store_is_under(&dir) {
            return Err(PathGrantError::CredentialPath(dir));
        }
        if is_secret_path_static(&dir) {
            return Err(PathGrantError::SecretPath(dir));
        }
        if write {
            self.check_write_grantable(&dir)?;
        }
        Ok(dir)
    }

    /// #1104 — every refusal a WRITE grant adds on top of the read rules.
    ///
    /// A read grant answers "let me show you this file". A write grant hands
    /// the session the ability to REPLACE bytes the operator will later run or
    /// trust, and outside the workspace there is no `git checkout` to undo it.
    /// So it refuses, in order:
    ///
    /// 1. **An unconfined backend.** Gated on the policy's own
    ///    `fs_confinement_backend`, which is fail-safe `None`. See the field.
    /// 2. **Anything that runs itself.** A start-up / hook / service directory,
    ///    in either direction (the grant is inside one, or contains one).
    /// 3. **Anything already runnable inside it.** One bounded walk refuses a
    ///    root holding an executable, a secret, or a `.git` (whose `hooks/`
    ///    and `config` are write-to-RCE and are NOT covered by
    ///    [`is_repo_control_path`](Self::is_repo_control_path), which is
    ///    deliberately workspace-scoped).
    ///
    /// Rule 3 is a grant-TIME refusal rather than a per-write deny on purpose.
    /// A write-deny inside a granted root would have to be expressed to the OS
    /// sandbox, and `SandboxManifest` has no `fs_write_deny` — so it would hold
    /// only for the in-process file tools and fail open for `Bash`, which is
    /// two answers to one question. Refusing the ROOT is one answer, enforced
    /// by the same `fs_write_allow` bind that enforces everything else.
    fn check_write_grantable(&self, dir: &Path) -> Result<(), PathGrantError> {
        if self.fs_confinement_backend.is_none() {
            return Err(PathGrantError::WriteRequiresConfinedBackend(
                dir.to_path_buf(),
            ));
        }
        if let Some(hit) = auto_run_overlap(dir) {
            return Err(PathGrantError::WriteRootAutoRun(hit));
        }
        scan_write_root(dir)
    }
}

/// Locations whose contents the OS, the shell or a VCS runs WITHOUT the user
/// asking — the write-to-RCE surface a folder grant must never cover.
///
/// Relative to the user's home. Both platform families are live on every host
/// rather than `cfg`-selected, for the same reason
/// `bash::policy::ALWAYS_GRANTED_PREFIXES` keeps both path syntaxes live: a
/// Windows-shaped relative path cannot collide with a real POSIX directory,
/// the only consequence of a spurious match is a refusal (this list's SAFE
/// direction), and keeping them live is what lets the Windows rules be graded
/// by the suite on Linux and macOS instead of only on the one platform CI
/// cannot introspect.
const AUTO_RUN_HOME_DIRS: &[&str] = &[
    // macOS
    "Library/LaunchAgents",
    "Library/LaunchDaemons",
    "Library/Application Support/com.apple.backgroundtaskmanagementagent",
    // Linux / freedesktop
    ".config/autostart",
    ".config/systemd/user",
    ".config/environment.d",
    ".local/share/systemd/user",
    // On PATH for the login shell on every unix desktop.
    ".local/bin",
    "bin",
    // Windows
    "AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Startup",
];

/// The absolute siblings of [`AUTO_RUN_HOME_DIRS`]. Same both-families rule.
const AUTO_RUN_SYSTEM_DIRS: &[&str] = &[
    "/Library/LaunchAgents",
    "/Library/LaunchDaemons",
    "/etc/systemd/system",
    "/etc/systemd/user",
    "/etc/init.d",
    "/etc/cron.d",
    "/etc/cron.daily",
    "/etc/profile.d",
    "/usr/lib/systemd/system",
    "C:/ProgramData/Microsoft/Windows/Start Menu/Programs/StartUp",
];

/// A directory whose contents git executes on ordinary developer commands.
/// Named here because [`WorkspacePolicy::is_repo_control_path`] is deliberately
/// WORKSPACE-scoped — a `.git` elsewhere on the host is not that predicate's
/// business, and a granted folder is exactly "elsewhere on the host".
const VCS_CONTROL_DIR: &str = ".git";

/// Extensions the OS, a shell, or a double-click will EXECUTE. Checked on
/// every platform (see [`AUTO_RUN_HOME_DIRS`] for why both families stay live);
/// on unix the owner-execute bit is checked as well, which is what catches an
/// ELF binary or a `+x` script with no extension at all.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "bat", "cmd", "com", "msi", "msix", "appx", "ps1", "psm1", "scr", "vbs", "vbe", "js",
    "jse", "wsf", "wsh", "lnk", "jar", "appimage", "dmg", "pkg", "deb", "rpm", "run", "apk", "app",
];

/// How many directory entries [`scan_write_root`] will look at before it gives
/// up and refuses.
///
/// A bound is required: the scan is on the interactive path of a person who
/// just clicked a button, and a home-sized tree would hang it. Exhausting the
/// budget is a REFUSAL, not a pass — the scan cannot prove the absence of an
/// executable it never reached, and "could not check" must never read as
/// "checked and clean".
const WRITE_GRANT_SCAN_BUDGET: usize = 20_000;

/// True when `dir` and `known` overlap in EITHER direction.
///
/// Both directions matter and they fail differently: granting
/// `~/.config/autostart` hands over the auto-run directory itself, while
/// granting `~/.config` hands over a directory that CONTAINS it. The second is
/// the one a containment check written as a single `starts_with` misses, and it
/// is the more likely user request of the two.
fn paths_overlap(dir: &Path, known: &Path) -> bool {
    dir.starts_with(known) || known.starts_with(dir)
}

/// The first auto-run location `dir` overlaps, if any.
///
/// Lexical and cheap, and deliberately runs BEFORE the walk: a directory that
/// overlaps an auto-run location must be refused whether or not it currently
/// has anything in it, and an empty `~/.config/autostart` is still the place a
/// `.desktop` file would be written to.
fn auto_run_overlap(dir: &Path) -> Option<PathBuf> {
    // A path that is INSIDE a `.git` is the hooks surface reached from below.
    // Component-wise, not `contains`, so a directory honestly named
    // `my.git-notes` is not mistaken for one.
    if dir
        .components()
        .any(|component| component.as_os_str() == VCS_CONTROL_DIR)
    {
        return Some(dir.to_path_buf());
    }
    let mut known: Vec<PathBuf> = AUTO_RUN_SYSTEM_DIRS.iter().map(PathBuf::from).collect();
    if let Some(home) = dirs::home_dir() {
        // #383 c3 — resolver: `canon_for_scope`. `$HOME` always exists, so the
        // two resolvers cannot disagree about it; the cheap one is chosen
        // because there is nothing here for the deep one to resolve.
        let home = canon_for_scope(&home);
        known.extend(
            AUTO_RUN_HOME_DIRS
                .iter()
                .map(|relative| home.join(relative)),
        );
    }
    known
        .into_iter()
        .find(|candidate| paths_overlap(dir, candidate))
}

/// True when this directory entry is something the OS or a shell would run.
fn entry_is_executable(path: &Path, metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_file() {
        return false;
    }
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            EXECUTABLE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
    {
        return true;
    }
    // The surviving `cfg` block is this function's tail expression; the other
    // is stripped before type-checking, exactly as `sandbox_root_identity`
    // does it in `vfs.rs`.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The bounded walk behind rule 3 of
/// [`check_write_grantable`](WorkspacePolicy::check_write_grantable).
///
/// Symlinks are NOT followed, and that is a security decision rather than a
/// loop-avoidance one: a symlink inside the granted root pointing outside it
/// is not a way to write outside, because both enforcement layers resolve
/// before they compare — `SandboxedFs` canonicalizes and finds the target
/// outside every grant, and the OS sandbox never bound the target's directory
/// writable in the first place. So the link's TARGET is not this scan's
/// business; the link itself is a symlink, not a regular file, and cannot be
/// the executable the operator later runs.
fn scan_write_root(dir: &Path) -> Result<(), PathGrantError> {
    scan_write_root_bounded(dir, WRITE_GRANT_SCAN_BUDGET)
}

/// [`scan_write_root`] with the budget injected.
///
/// Separate so the exhaustion arm is graded with a budget of 2 instead of a
/// twenty-thousand-file fixture. A test that is too slow to run is a test that
/// gets `#[ignore]`d, and an `#[ignore]`d refusal is not a refusal.
fn scan_write_root_bounded(dir: &Path, limit: usize) -> Result<(), PathGrantError> {
    let unscannable = |path: &Path, error: std::io::Error| {
        PathGrantError::WriteRootUnscannable(path.to_path_buf(), error.to_string())
    };
    let mut budget = limit;
    let mut queue = vec![dir.to_path_buf()];
    while let Some(current) = queue.pop() {
        let entries = std::fs::read_dir(&current).map_err(|error| unscannable(&current, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| unscannable(&current, error))?;
            if budget == 0 {
                return Err(PathGrantError::WriteRootTooLarge(dir.to_path_buf(), limit));
            }
            budget -= 1;
            let path = entry.path();
            // `symlink_metadata`, never `metadata`: the latter follows the
            // link and would classify `<granted>/notes -> /bin/sh` as an
            // executable regular file living in the grant, which it is not.
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|error| unscannable(&path, error))?;
            if metadata.is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if path.file_name().is_some_and(|name| name == VCS_CONTROL_DIR) {
                    return Err(PathGrantError::WriteRootAutoRun(path));
                }
                queue.push(path);
                continue;
            }
            if is_secret_path_static(&path) {
                return Err(PathGrantError::WriteRootSecret(path));
            }
            if entry_is_executable(&path, &metadata) {
                return Err(PathGrantError::WriteRootExecutable(path));
            }
        }
    }
    Ok(())
}

/// Is there room for a grant on `dir` conferring at least `write`?
///
/// `Ok(true)` — a LIVE grant already confers this access, so recording a
/// second entry would be a duplicate. An expired grant does not count as
/// cover, or a deadline could be silently extended by re-asking.
/// `Ok(false)` — not covered, and under the cap.
///
/// #1104 — coverage is ACCESS-AWARE, and the read-only spelling of this was a
/// silent downgrade waiting to happen: a session already holding a READ grant
/// on `~/Downloads` that is then granted WRITE on the same folder would have
/// matched here, returned "already covered", recorded nothing, and reported
/// success. The user would have been told write was granted and the very next
/// write would have been refused. A read grant is not cover for a write.
fn grant_capacity(
    grants: &[SessionPathGrant],
    dir: &Path,
    now: SystemTime,
    write: bool,
) -> Result<bool, PathGrantError> {
    if grants
        .iter()
        .any(|existing| existing.confers(now, write) && dir.starts_with(&existing.root))
    {
        return Ok(true);
    }
    if grants.iter().filter(|g| g.is_live(now)).count() >= MAX_SESSION_READ_GRANTS {
        return Err(PathGrantError::CapReached(MAX_SESSION_READ_GRANTS));
    }
    Ok(false)
}

impl PathGrantSink for WorkspacePolicy {
    fn grant_path(&self, root: &Path, write: bool) -> bool {
        match self.grant_session_read_root(root, write) {
            Ok(dir) => {
                tracing::info!(
                    root = %dir.display(),
                    write,
                    "session path grant recorded"
                );
                true
            }
            Err(error) => {
                // Deliberately not `warn!`: with `RUST_LOG` unset only ERROR
                // reaches stderr, so a warning here would reach nobody. The
                // person just clicked a button and is owed the reason it did
                // not take.
                eprintln!("wayland: folder access not granted - {error}");
                false
            }
        }
    }
}

/// True when granting `dir` would hand over a credential store that lives
/// beneath it. The sibling of [`path_is_in_credential_store`], which only
/// catches the store-or-below direction: granting `~/Projects` is fine, but
/// granting a directory that CONTAINS `~/.ssh` is the same disclosure as
/// granting `~/.ssh` itself.
fn credential_store_is_under(dir: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    // #383 c3 — resolver: `canon_for_scope`. `$HOME` always exists, so the
    // two resolvers cannot disagree about it; the cheap one is chosen
    // because there is nothing here for the deep one to resolve.
    let home = canon_for_scope(&home);
    CREDENTIAL_STORES
        .iter()
        .any(|relative| home.join(relative).starts_with(dir))
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

/// The static, lexical sibling of [`WorkspacePolicy::is_vcs_content_store`],
/// for the one caller that has no policy instance to ask: `grep_policy`
/// (SR-05).
///
/// FerroxLabs/wayland-core#244 c3 asks that the VCS content store be
/// "unreachable to a shell subprocess". `BashTool`'s subprocess is confined by
/// the OS sandbox, which consumes [`vcs_content_stores`] as `fs_read_deny` —
/// WHERE THE BACKEND ENFORCES READ-DENY. Where it does not, the shell is
/// refused outright, except for the local operator, who keeps an unconfined
/// one; on the Windows job-object default that is the ordinary interactive
/// user, and there the store IS readable to `Bash`. That is a decided
/// position (`.planning/DECISIONS.md`, Q-391), measured and pinned by
/// `crates/wcore-tools/tests/bash_vcs_store_local_operator_gap.rs`, and
/// tracked as FerroxLabs/wayland-core#391 — not an oversight, and not
/// something this predicate changes either way.
/// `GrepTool` spawns its OWN subprocess (`rg` / `grep` / `findstr`) through
/// `shell_command_argv`, OUTSIDE that sandbox, so none of that deny list ever
/// reaches it — and its only other predicate, [`is_secret_path_static`],
/// matches credential NAMES while a loose object is named after its hash.
///
/// MEASURED at integ/f13 a278f8c3b before this existed, with the production
/// `SandboxedFs::new(SecretDenyFs::new(RealFs, WorkspacePolicy::contained))`
/// stack: `Grep(pattern, path = ".git")` returned
/// `.git/lfs/objects/aa/bb/deadbeef:1:WLCANARY-LFSOBJ-244` and
/// `.git/objects/ab/cd1234:1:WLCANARY-ROOTOBJ-244` in PLAINTEXT, while naming
/// the store outright was refused. `ctx.vfs.exists()` gates only the TOP-LEVEL
/// `path` argument and `.git` is not itself a store, so naming the control
/// directory ONE COMPONENT ABOVE it walked straight in.
///
/// The any-depth LEXICAL arm only ([`inside_vcs_store`]) — the same arm
/// [`WorkspacePolicy::is_vcs_content_store`] tries first. It is HALF of that
/// predicate and must never be mistaken for all of it.
///
/// This doc previously claimed "Grep never sees the second arm's targets: a
/// gitfile-pointed or `alternates`-borrowed store lives OUTSIDE the workspace
/// root". That sentence was FALSE and the leak it excused is MEASURED in
/// `crates/wcore-tools/tests/grep_vcs_named_store_deny.rs`. `git init
/// --separate-git-dir mygit` and `git clone --reference ../shared` both resolve
/// arm 2 to a directory INSIDE the root, where nothing is lexically a store:
///
/// ```text
/// ./notes.txt:1:WLCANARY-CONTROL-OK
/// ./mygit/objects/ab/cd1234:1:WLCANARY-GITFILE-244
/// ./shared-objects/ab/cd1234:1:WLCANARY-ALTERNATES-244
/// ```
///
/// — returned by `Grep(".")`, the whole-workspace search, in the same
/// `Contained` posture whose in-process VFS refuses those exact bytes.
///
/// So `grep_policy` asks BOTH arms: this one per path, and
/// [`vcs_content_stores`] resolved for the workspace root and for every
/// directory the walk traverses. Grep may afford that discovery because it
/// TRAVERSES, where a point-predicate answering one path at a time cannot —
/// which makes Grep deliberately STRICTER than
/// [`WorkspacePolicy::is_vcs_content_store`] on a VENDORED gitfile, whose store
/// arm 2 (root's `.git` only) never sees. Denying a real object store is not a
/// wrong refusal, so the strictness is safe in the one direction it goes; the
/// VFS-side remainder is FerroxLabs/wayland-core#390.
pub fn is_vcs_content_store_static(path: &Path) -> bool {
    inside_vcs_store(path)
}

/// Free-function body of `is_secret_path` (uses no `self` fields). Extracted
/// so `compute_secret_deny` can call it without a `WorkspacePolicy` instance.
/// The one credential-file name predicate in the crate. `Read`/`SecretDenyFs`
/// reach it via [`WorkspacePolicy::is_secret_path`]; `grep_policy` (SR-05) uses
/// it directly, because Grep has no policy instance and must not grow a second,
/// divergent copy of this list.
///
/// `pub` rather than crate-private because `wcore-cli`'s `@`-attach guard
/// (`tui::commands::at_ref_guard::is_secret_path`) unions its own file-name
/// rules with this one. It used to keep a private copy, and the two lists had
/// drifted apart in BOTH directions — the divergence is the defect the union
/// closes, and a second copy is what re-opens it.
pub fn is_secret_path_static(path: &Path) -> bool {
    // ASCII case is folded on EVERY rule, on EVERY platform — not just on the
    // extension, which is how this was written and how `.ENV` escaped while
    // `server.KEY` did not.
    //
    // On macOS and Windows (the two hosts the desktop app ships on) the
    // filesystem is case-INSENSITIVE, so `.ENV` and `.env` are the SAME FILE
    // and a case-sensitive denylist is simply bypassable by spelling.
    //
    // On LINUX the filesystem IS case-sensitive, so `.ENV` is genuinely a
    // different file and folding case here can over-deny. That is the right
    // trade anyway, and it is taken deliberately rather than by omission:
    //
    //   * This is a DENY list. Over-denying refuses a read — visible,
    //     recoverable, the operator picks another path. Under-denying hands a
    //     credential to the model — silent and irreversible. The asymmetry is
    //     not close.
    //   * One predicate feeds `Read`, `Grep`, `SecretDenyFs` AND the OS-sandbox
    //     deny-set computation. A `cfg(unix)`-split answer would make the
    //     sandbox's deny list disagree with the in-process check on the same
    //     path — exactly the seam holes appear in — and would make a Linux CI
    //     run stop grading the macOS/Windows behaviour.
    //   * The cost is bounded by the lists themselves: nothing in
    //     SECRET_BASENAMES / SEGMENTS / SUFFIXES / EXTENSIONS has a legitimate
    //     upper-case homonym a user would be blocked from reading. `ID_RSA` on
    //     Linux is an SSH key that shouted, not a source file.
    //   * `SECRET_EXTENSIONS` has folded case on Linux since it was written,
    //     and `wcore-cli`'s `@`-attach guard folds case on every rule. Folding
    //     here makes the codebase consistent instead of splitting it.
    //
    // Allocation: one `String` for the whole path (down from two — the old
    // `replace` plus a per-call `to_ascii_lowercase` of the extension). The
    // file-name rules match through `*_ci` helpers, which allocate nothing.
    // This runs on every tool call.
    let raw = path.to_string_lossy();
    let mut s = String::with_capacity(raw.len());
    for c in raw.chars() {
        // ASCII lower-casing never changes a char's UTF-8 length, so the
        // capacity above is exact and `s` stays byte-aligned with `raw`.
        s.push(if c == '\\' {
            '/'
        } else {
            c.to_ascii_lowercase()
        });
    }
    // Trailing spaces/dots come off the WHOLE path too, not just the derived
    // file name below. The suffix rules (`/.env`, `/.npmrc`, `/.aws/credentials`)
    // match against this string, so without this `.env ` fails every one of
    // them while `.env` matches — which is the bypass, just relocated.
    // Trailing bytes of the path ARE the trailing bytes of its final
    // component, so trimming the end is exactly the right scope.
    let s = s.trim_end_matches([' ', '.']);

    // Win32 STRIPS trailing spaces and dots from the final path component
    // before opening it, so `.env `, `.env.` and `.env. ` all open `.env`.
    // A denylist that matches the literal name is therefore bypassable by
    // typing a space, exactly as it was bypassable by typing a capital before
    // the case fold above. Same deny-list asymmetry, same answer: strip them
    // on every platform. On Linux `.env ` is genuinely a distinct file, so
    // this can over-deny by one pathological name — which refuses a read,
    // against under-denying which hands over a credential.
    //
    // `trim_end_matches` returns a borrowed slice, so this allocates nothing.
    let effective_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.trim_end_matches([' ', '.']))
        .filter(|n| !n.is_empty());

    // The extension is re-derived from the EFFECTIVE name rather than taken
    // from `path.extension()`: for `foo.key ` the real extension is `key `,
    // which matches nothing in SECRET_EXTENSIONS.
    if let Some(ext) = effective_name
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, ext)| ext)
        && SECRET_EXTENSIONS
            .iter()
            .any(|e| ext.eq_ignore_ascii_case(e))
    {
        return true;
    }
    if let Some(name) = effective_name {
        if SECRET_BASENAMES
            .iter()
            .any(|b| name.eq_ignore_ascii_case(b))
        {
            return true;
        }
        // A named SSH key (`deploy_rsa`) — the stem varies, the tail does not.
        if SECRET_NAME_SUFFIXES.iter().any(|s| ends_with_ci(name, s)) {
            return true;
        }
        // service-account*.json, bare key.json, and separator-bounded *-key.json / *_key.json.
        // Does NOT match monkey.json, turnkey.json, hotkey.json (no false positives).
        if ends_with_ci(name, ".json")
            && (starts_with_ci(name, "service-account")
                || name.eq_ignore_ascii_case("key.json")
                || ends_with_ci(name, "-key.json")
                || ends_with_ci(name, "_key.json"))
        {
            return true;
        }
        // terraform.tfstate and terraform.tfstate.backup (compound extension)
        if contains_ci(name, ".tfstate") {
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

// ASCII-case-insensitive `starts_with` / `ends_with` / `contains` for the final
// path component in `is_secret_path_static`. Byte-wise, so a multi-byte
// character can never land the comparison on a UTF-8 boundary and panic, and
// allocation-free — the predicate runs on every tool call. Every `needle` here
// is a lower-case ASCII literal from the denylists above.
fn starts_with_ci(hay: &str, needle: &str) -> bool {
    hay.len() >= needle.len()
        && hay.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle.as_bytes())
}

fn ends_with_ci(hay: &str, needle: &str) -> bool {
    hay.len() >= needle.len()
        && hay.as_bytes()[hay.len() - needle.len()..].eq_ignore_ascii_case(needle.as_bytes())
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.len() >= needle.len()
        && hay
            .as_bytes()
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
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
    dirs: &mut Vec<(PathBuf, SystemTime)>,
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
        out.extend(project_committed_secrets(root, readable_canon, dirs));
    }

    out.sort();
    out.dedup();
    out
}

thread_local! {
    /// #1111 acceptance 1 asked for the walk to be counted "via an injected
    /// counter, not wall-clock". This is that counter. It is THREAD-LOCAL on
    /// purpose: `cargo test` runs one binary as threads in a single process, so
    /// a process-global would be corrupted by any concurrent test that also
    /// walks and the count would quietly stop meaning anything. Every
    /// `project_committed_secrets` call runs on the thread that asked for it.
    static WALK_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };

    /// #1182: how many filesystem entries the walks on this thread actually
    /// VISITED. Thread-local for the same reason as `WALK_CALLS`, and
    /// attributed to the thread that ASKED for the walk even when the parallel
    /// arm does the visiting on a pool — the question a caller asks is "did the
    /// operation I just performed enumerate the tree", and an answer that
    /// depended on which worker happened to see an entry would not be one.
    static WALK_ENTRIES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Number of full workspace secret walks ([`project_committed_secrets`])
/// performed **on the calling thread** since the process started.
///
/// Read it either side of an operation to assert how many walks that operation
/// really cost. It counts WALKS, not entries: one call that starts serial and
/// restarts in parallel above `SERIAL_WALK_BUDGET` is one walk, because the
/// question #1111 asks is whether a walk is repeated.
///
/// Graded by `tests/secret_walk_call_count_test.rs`, which also carries the
/// executable evidence for why this walk is deliberately NOT memoised.
pub fn walk_calls() -> u64 {
    WALK_CALLS.with(|c| c.get())
}

/// Number of filesystem entries the secret walks requested **on the calling
/// thread** have visited since the process started.
///
/// Read it either side of an operation to assert, by DIRECT OBSERVATION, how
/// much of the tree that operation enumerated. `walk_calls` answers "was the
/// walk entered"; this answers "did it enumerate the tree", which is the half a
/// caller needs when it is asserting that some other operation did NOT walk.
///
/// #1182: `contained_construction_does_not_walk_the_workspace` used to
/// establish that its instrument was alive with a WALL-CLOCK RATIO (the real
/// tree's walk had to be measurably slower than an empty tree's). Under load
/// the two timings compress and the control declared itself dead. Counting the
/// entries removes the timing from the question entirely while keeping the
/// property that mattered: a walk that became unreachable reports ZERO here and
/// still fails the test.
///
/// Counts the arm that produced the answer. An oversized tree re-walks its
/// first `SERIAL_WALK_BUDGET` entries in parallel; only the parallel arm's
/// entries are counted, so the prefix is not double-charged.
pub fn walk_entries() -> u64 {
    WALK_ENTRIES.with(|c| c.get())
}

/// Absolute, canonicalized paths of the workspace's OWN committed secrets
/// (`.env`, `service-account*.json`, `*.pem`, …) that are reachable from a
/// sandbox mounted at `root`. Walks `root` ignoring `.gitignore` (a
/// gitignored `.env` must still be denied) and emits a path only when it is
/// under a readable/mounted root. Shared by `compute_secret_deny` (Contained)
/// and `WorkspacePolicy::with_project_secret_deny` (#667, Full/remote Trusted)
/// so the two paths cannot drift.
///
/// The returned list is SORTED. A big tree is walked in parallel (below), and a
/// security boundary must not vary with thread scheduling.
fn project_committed_secrets(
    root: &Path,
    readable_canon: &[PathBuf],
    dirs: &mut Vec<(PathBuf, SystemTime)>,
) -> Vec<PathBuf> {
    WALK_CALLS.with(|c| c.set(c.get() + 1));

    let system_roots: Vec<PathBuf> = SYSTEM_CREDENTIAL_STORES.iter().map(PathBuf::from).collect();
    let under_mounted = |p: &Path| {
        readable_canon.iter().any(|r| p.starts_with(r))
            || system_roots.iter().any(|r| p.starts_with(r))
    };

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
    let builder = || {
        let mut walker = ignore::WalkBuilder::new(root);
        walker
            .standard_filters(false) // a .gitignore'd .env must still be denied
            .hidden(false)
            .follow_links(false);
        walker
    };

    // What the prefilter leaves behind is `readdir` over the whole tree, and on
    // a big workspace that IS the cost — #1113 measured ~90 ms per exec on a
    // 90k-entry tree, paid by every sub-agent because the spawner runs them
    // `contained`. Threads are the fix, but they are not free to start
    // (measured on a 96-core host: 0.07 ms for a serial walk of an empty tree
    // versus a ~1.7 ms floor for a parallel one, whatever the thread count), so
    // a small workspace must not have to pay for them. Walk serially until the
    // tree proves it is big enough to be worth a thread pool.
    {
        let mut out: Vec<PathBuf> = Vec::new();
        // #1111: held aside rather than appended straight to `dirs`, because the
        // oversized arm below abandons this prefix and re-walks it in parallel —
        // and a stamp that listed the same directory twice would still be
        // correct but would pay for it on every revalidation.
        let mut stamp: Vec<(PathBuf, SystemTime)> = Vec::new();
        let mut visited = 0usize;
        let mut oversized = false;
        for result in builder().build() {
            visited += 1;
            if visited > SERIAL_WALK_BUDGET {
                oversized = true;
                break;
            }
            if let Ok(entry) = result {
                if let Some(stamped) = dir_stamp(&entry) {
                    stamp.push(stamped);
                }
                if let Some(secret) = secret_entry(&entry, &under_mounted) {
                    out.push(secret);
                }
                if let Some(store) = vcs_store_entry(&entry, &under_mounted) {
                    out.push(store);
                }
            }
        }
        if !oversized {
            out.sort();
            dirs.append(&mut stamp);
            WALK_ENTRIES.with(|c| c.set(c.get() + visited as u64));
            return out;
        }
    }

    // Oversized: start again in parallel. The prefix above is re-walked, but it
    // is bounded by `SERIAL_WALK_BUDGET` and every entry in it is already in
    // the page cache, so it costs a fraction of the threads it just justified.
    //
    // Threads change the ORDER entries are visited in and which thread a
    // `canonicalize` lands on — never WHICH entries are visited: `WalkParallel`
    // applies the same filters as `Walk`, and both arms classify through the
    // same `secret_entry`. Sorting below removes the arrival order, so the
    // answer is identical run to run and identical to the serial arm's. Pinned
    // by `tests/walk_parallel_identity_test.rs`.
    let found = Mutex::new(Vec::<PathBuf>::new());
    let walked = Mutex::new(Vec::<(PathBuf, SystemTime)>::new());
    // #1182: entries are counted into a per-CALL atomic and folded into the
    // calling thread's `WALK_ENTRIES` once the pool has joined, so the count a
    // caller reads is the one its own operation caused.
    let visited_parallel = std::sync::atomic::AtomicU64::new(0);
    builder().build_parallel().run(|| {
        Box::new(|result| {
            visited_parallel.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // An unreadable directory is skipped here exactly as the serial
            // arm's `Err` skips it.
            if let Ok(entry) = result {
                if let Some(stamped) = dir_stamp(&entry) {
                    walked.lock().expect(POISONED).push(stamped);
                }
                if let Some(secret) = secret_entry(&entry, &under_mounted) {
                    found.lock().expect(POISONED).push(secret);
                }
                if let Some(store) = vcs_store_entry(&entry, &under_mounted) {
                    found.lock().expect(POISONED).push(store);
                }
            }
            ignore::WalkState::Continue
        })
    });
    let mut out = found.into_inner().expect(POISONED);
    out.sort();
    dirs.extend(walked.into_inner().expect(POISONED));
    WALK_ENTRIES
        .with(|c| c.set(c.get() + visited_parallel.load(std::sync::atomic::Ordering::Relaxed)));
    out
}

/// #1111 — the directory half of the walk's stamp.
///
/// `None` for anything that is not a directory. A directory whose mtime the
/// platform will not report is stamped with `UNIX_EPOCH`, a value
/// [`WorkspacePolicy::deny_cache_hit`] refuses to match: an unstampable
/// directory must DISABLE the memo, never sit inside a stamp as a hole that
/// revalidation silently steps over.
fn dir_stamp(entry: &ignore::DirEntry) -> Option<(PathBuf, SystemTime)> {
    if !entry.file_type().is_some_and(|t| t.is_dir()) {
        return None;
    }
    // `std::fs::symlink_metadata`, NOT `entry.metadata()`, and the difference is
    // load-bearing rather than stylistic: revalidation reads the mtime with
    // `symlink_metadata`, and a stamp is only meaningful if both sides use the
    // SAME instrument. On Windows they are not interchangeable — a walk's
    // `DirEntry` carries the timestamps the parent directory's enumeration
    // returned, and NTFS updates that cached copy lazily, so the enumerated
    // value routinely differs from the one an open of the directory reports.
    // Stamping from the enumeration made every revalidation mismatch and the
    // memo never hit on Windows; caught by `two_execs_perform_exactly_one_walk`
    // running on a real Windows host, not by reading this code.
    let mtime = std::fs::symlink_metadata(entry.path())
        .ok()
        .and_then(|meta| meta.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Some((entry.path().to_path_buf(), mtime))
}

/// Entries the serial arm of [`project_committed_secrets`] will visit before it
/// hands the tree to the parallel one.
///
/// A tree this size finishes serially in less time than a thread pool takes to
/// start; a bigger one pays this budget twice over to win multiples back. NOT
/// load-bearing for correctness — both arms return the same set — only for
/// which one pays less. Swept on this host against
/// `secret_deny_paths_dynamic`, median of 7, warm:
///
/// ```text
///   budget    empty   100 files   3000 dirs   15k files   134k entries
///      128   0.136ms     0.490ms     11.95ms     10.33ms       117.7ms
///      256   0.137ms     0.488ms     13.75ms     10.59ms       120.5ms
///     1024   0.137ms     0.487ms     17.26ms     14.30ms       130.2ms
/// ```
///
/// `pub` ONLY so `tests/walk_parallel_identity_test.rs` can prove its parallel
/// fixture actually crosses this threshold. A fixture that silently sits under
/// it grades the serial arm twice and leaves the parallel arm ungraded.
#[doc(hidden)]
pub const SERIAL_WALK_BUDGET: usize = 256;

/// #1145 - is an observed mtime old enough, relative to the instant a scan
/// started, to be evidence that nothing changed during the scan?
///
/// Shared by [`WorkspacePolicy::deny_cache_hit`] and
/// [`WorkspacePolicy::vcs_store_cache_hit`] so the two memos in this file
/// cannot answer the freshness question differently.
///
/// Take the granularity from the stamp actually in hand rather than assuming
/// the build host's: a filesystem that resolves only whole seconds reports a
/// zero nanosecond part for every mtime it stamps. A sub-second stamp that
/// happens to land exactly on a second boundary is misread here, and falls in
/// the conservative direction - a rescan that was not strictly needed, never a
/// hit that was not earned.
fn stamp_is_settled(observed: SystemTime, stamped_at: SystemTime) -> bool {
    let granularity = if observed
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.subsec_nanos())
        == 0
    {
        WHOLE_SECOND_MTIME_GRANULARITY
    } else {
        SUBSECOND_MTIME_GRANULARITY
    };
    stamped_at
        .duration_since(observed)
        .is_ok_and(|behind| behind > granularity)
}

/// The lock is taken only to push a path that has already been canonicalized,
/// so nothing that can panic runs while it is held and this cannot fire.
const POISONED: &str = "secret-deny walk mutex poisoned";

/// The DIRECTORY half of the per-entry decision of
/// [`project_committed_secrets`]: a VCS content store found at any depth under
/// the walk root (#322).
///
/// Emits the store DIRECTORY, never its members. That is the whole reason the
/// fix lives here rather than in [`is_secret_path_static`] / [`secret_entry`]:
/// a real `.git/objects` holds hundreds of thousands of files, the walk
/// deliberately does not prune, and classifying object FILES as secrets would
/// buy one deny-list entry and one symlink-resolving `canonicalize` syscall per
/// object. One entry per store is the same denial at a bounded cost, and it is
/// exactly what the OS backends already consume from [`vcs_content_stores`].
///
/// Shared verbatim by the serial and parallel arms, like [`secret_entry`].
fn vcs_store_entry(
    entry: &ignore::DirEntry,
    under_mounted: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if !entry.file_type().is_some_and(|t| t.is_dir()) || !is_vcs_store_dir(entry.path()) {
        return None;
    }
    std::fs::canonicalize(entry.path())
        .ok()
        .filter(|canon| under_mounted(canon))
}

/// The per-entry decision of [`project_committed_secrets`], shared verbatim by
/// its serial and parallel arms so the two cannot answer differently.
fn secret_entry(
    entry: &ignore::DirEntry,
    under_mounted: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let path = entry.path();
    if !entry.path_is_symlink() {
        // Regular file: cheap lexical check on the raw name FIRST; only a
        // secret-named file is worth the canonicalize syscall.
        if !entry.file_type().is_some_and(|t| t.is_file()) || !is_secret_path_static(path) {
            return None;
        }
        return std::fs::canonicalize(path)
            .ok()
            .filter(|canon| under_mounted(canon));
    }
    // Symlink (rare): resolve the target and deny the link's own canonical
    // path if the TARGET is a secret, masking a benign-named link to a secret
    // (`notes.txt` → `.env`). Must canonicalize regardless of the link's name.
    // External-target residual (target not under a mounted root) is documented
    // in the plan — backstopped by network-Deny.
    std::fs::canonicalize(path)
        .ok()
        .filter(|canon| is_secret_path_static(canon) && under_mounted(canon))
}

/// Is `candidate`'s subtree already visited in full by a walk rooted at
/// `covering`?
///
/// Prefix containment alone is NOT the question. The covering walk descends by
/// `readdir`, so a directory between the two that it cannot LIST hides
/// `candidate` from it — an execute-only `0o111` directory is traversable, so
/// `candidate`'s own walk would still find everything under it. Answering
/// `false` there keeps the deduplication a pure win: an unreachable subtree is
/// walked exactly as it is today. Both paths are canonical (grants are
/// canonicalized when minted, the workspace root by `canon`), so this is a
/// component comparison with no symlink left to resolve.
fn walk_root_is_covered(covering: &Path, candidate: &Path) -> bool {
    let Ok(rest) = candidate.strip_prefix(covering) else {
        return false;
    };
    let mut dir = covering.to_path_buf();
    let mut components = rest.components().peekable();
    loop {
        // `read_dir` only opens the directory handle; it does not enumerate.
        if std::fs::read_dir(&dir).is_err() {
            return false;
        }
        match components.next() {
            // The last component is `candidate` itself: whether IT can be
            // listed constrains both walks equally, so it is not checked here.
            Some(_) if components.peek().is_none() => return true,
            Some(component) => dir.push(component),
            None => return true,
        }
    }
}

/// VCS CONTENT stores under `root` that must be OS-sandbox-denied for reads in a
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
///
/// #243 extends the same mechanism to the other VCSes a project may actually be
/// checked out under: Mercurial revlogs (`.hg/store`), Subversion pristine text
/// bases (`.svn/pristine`) and Bazaar's packed store (`.bzr/repository`) each
/// reconstruct a committed secret through their own porcelain (`hg cat -r`,
/// `svn cat -r`, `bzr cat -r`) exactly as `git show` does. Working-state files
/// that carry no content — `.hg/dirstate`, `.svn/wc.db`, `.git/refs` — stay
/// readable, mirroring the `git rev-parse` carve-out above.
///
/// #242 resolves a `.git` FILE (a "gitfile"). A linked worktree (`git worktree
/// add`) and a submodule checkout both have one, and the store it names can sit
/// OUTSIDE `root` — where `root.join(".git/objects")` does not exist, so the
/// deny above silently covers nothing. See [`gitfile_content_stores`].
pub(crate) fn vcs_content_stores(root: &Path) -> Vec<PathBuf> {
    scan_vcs_content_stores(root).stores
}

/// One scan of the VCS content stores under `root`: the stores, the paths whose
/// state decided that answer, and what the scan cost.
///
/// FerroxLabs/wayland-core#376. `vcs_content_stores` was called once per
/// ordinary-path VFS operation and rebuilt all of this from the filesystem
/// every time. Reporting the WITNESSES from the same code that reads them is
/// what lets the answer be memoised without a second, hand-maintained list of
/// "things that would invalidate it" — the shape that goes stale silently.
struct StoreScan {
    stores: Vec<PathBuf>,
    witnesses: Vec<(PathBuf, Option<SystemTime>)>,
    probes: u64,
    stamped_at: SystemTime,
}

impl StoreScan {
    fn new() -> Self {
        Self {
            stores: Vec::new(),
            witnesses: Vec::new(),
            // Taken BEFORE any probe: anything modified from here on is inside
            // the scan's own window and must not be trusted by a revalidation.
            stamped_at: SystemTime::now(),
            probes: 0,
        }
    }

    /// Stamp `path`, whose state decides part of the answer. Absent is a
    /// perfectly good stamp: it flips to `Some` the moment the path appears.
    ///
    /// Returns whether the path existed.
    fn witness(&mut self, path: PathBuf) -> bool {
        if let Some((_, stamp)) = self.witnesses.iter().find(|(seen, _)| *seen == path) {
            return stamp.is_some();
        }
        self.probes += 1;
        let meta = std::fs::symlink_metadata(&path).ok();
        let stamp = meta.as_ref().and_then(|meta| meta.modified().ok());
        let existed = stamp.is_some();
        let is_link = meta.is_some_and(|meta| meta.file_type().is_symlink());
        self.witnesses.push((path.clone(), stamp));
        if is_link {
            self.witness_link_target(&path);
        }
        existed
    }

    /// Stamp `path` only if it exists, for a path whose APPEARANCE is already
    /// covered by a witness that is kept.
    ///
    /// The four control directories are the case: `<root>/.hg` cannot come into
    /// being without moving `<root>`'s mtime, and `<root>` is always stamped.
    /// Dropping the three that a git checkout does not have takes the
    /// revalidation of an ordinary repository from six stats to three — and a
    /// revalidation that costs as much as the scan it replaces is not a cache.
    /// NOT applicable to a path outside the root (a gitfile's gitdir, a
    /// borrowed alternates store): nothing stamped would move if one appeared.
    ///
    /// Returns whether the path is THERE, so the caller can skip everything
    /// that could only exist underneath it.
    fn witness_if_present(&mut self, path: PathBuf) -> bool {
        self.probes += 1;
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return false;
        };
        let Ok(stamp) = meta.modified() else {
            return false;
        };
        if self.witnesses.iter().any(|(seen, _)| *seen == path) {
            return true;
        }
        self.witnesses.push((path.clone(), Some(stamp)));
        if meta.file_type().is_symlink() {
            self.witness_link_target(&path);
        }
        true
    }

    /// Every content store the control directories directly under `dir` name.
    ///
    /// FerroxLabs/wayland-core#394 c3 / #396 c3 / #398 c3. `grep_policy::
    /// scope_for` calls this once per directory it traverses -- through
    /// [`vcs_content_stores`] -- so what it costs on a directory holding NO
    /// control directory is the per-traversed-directory figure those three
    /// criteria pin, and every `Grep(".")` pays it at every directory of the
    /// tree.
    ///
    /// Two skips, both resting on the invariant the CALLER establishes by
    /// stamping `dir` itself: a control directory cannot come into being
    /// without moving `dir`'s mtime, so an ABSENT one needs no stamp of its
    /// own, and nothing can exist underneath it either.
    ///
    /// * an absent control directory's store leaves are not probed -- six
    ///   `symlink_metadata` calls on paths that cannot exist;
    /// * `gitfile_content_stores` and `alternate_object_dirs` both read
    ///   `<dir>/.git`, so neither can find anything when it is not there.
    ///
    /// And the control names are DEDUPLICATED before probing: `VCS_CONTENT_
    /// STORES` holds six rows over four control directories, `.git` three
    /// times, and `witness_if_present` counts its probe before it checks for a
    /// duplicate.
    ///
    /// MEASURED, differential `strace -f -c`, ordinary directory, interleaved
    /// arms: **17.000 -> 5.000** syscalls per call, which takes the pair
    /// `scope_for` pays per traversed directory from 25.000 to 13.000. The
    /// answer is unchanged in both directions -- nothing can be found under a
    /// directory that is not there -- and so is the witness set, because an
    /// absent control directory was never stamped anyway.
    fn scan_control_dirs_in(&mut self, dir: &Path) {
        let mut dot_git = false;
        let mut done: Vec<&str> = Vec::new();
        for (control_name, _) in VCS_CONTENT_STORES {
            if done.contains(control_name) {
                continue;
            }
            done.push(control_name);
            let control = dir.join(control_name);
            if !self.witness_if_present(control.clone()) {
                continue;
            }
            dot_git |= *control_name == ".git";
            for (owner, store) in VCS_CONTENT_STORES {
                if owner == control_name {
                    self.push_store(control.join(store));
                }
            }
        }
        if dot_git {
            self.gitfile_content_stores(dir);
            self.alternate_object_dirs(dir.join(".git/objects"));
        }
    }

    /// Stamp what a symlinked witness POINTS AT, not only the link itself.
    ///
    /// A link's own mtime does NOT move when its target gains a child. So a
    /// control directory reached through a symlink (`<root>/.git` pointed at
    /// `<root>/real-git`) could grow an object store with every witness
    /// unchanged: not `<root>` (the target directory already existed), not
    /// `<root>/.git` (the link's own mtime), not `objects/info/alternates`
    /// (still absent). [`WorkspacePolicy::vcs_store_cache_hit`] then returned
    /// the stale EMPTY list for the life of the process, and arm 1 cannot cover
    /// the gap either -- the canonical path is `<root>/real-git/objects`, whose
    /// parent component is not `.git`, so [`is_vcs_store_dir`] says no.
    /// MEASURED consequence before this: `Grep(path="real-git")` on the
    /// production contained stack returned `.git/lfs` plaintext, re-opening the
    /// whole of FerroxLabs/wayland-core#375 through #376's memo. A cache whose
    /// invalidation cannot observe the mutation is not a cache.
    ///
    /// The TARGET's mtime is the one that moves, so the target is what must be
    /// stamped. A link whose target does not exist YET cannot be canonicalized;
    /// the path it NAMES is stamped absent instead, and flips to present the
    /// moment the target appears -- a miss, which rescans.
    ///
    /// Costs nothing for an ordinary checkout, where no witness is a link.
    /// Graded by `vfs_guard_cost::a_store_under_a_symlinked_control_dir_created_after_the_scan_is_denied`.
    fn witness_link_target(&mut self, link: &Path) {
        self.probes += 1;
        if let Ok(target) = std::fs::canonicalize(link) {
            self.witness(target);
            return;
        }
        self.probes += 1;
        let Ok(named) = std::fs::read_link(link) else {
            return;
        };
        // `witness` de-duplicates by path, so a symlink CYCLE terminates on the
        // hop that comes back round rather than recursing forever.
        self.witness(resolve_against(link.parent().unwrap_or(link), named));
    }

    /// Canonicalize and record `p` when it exists. A path that does not exist
    /// is dropped rather than denied: the deny list is handed to the OS
    /// sandbox, and a deny for a non-existent path is noise the backend still
    /// has to carry.
    ///
    /// The CALLER stamps the directory whose mtime decides whether `p` is
    /// present, absent or re-pointed — one stamp for all of a control
    /// directory's store leaves rather than one per leaf, and the caller is the
    /// only one that knows whether that directory's own appearance is already
    /// covered.
    fn push_store(&mut self, p: PathBuf) {
        self.probes += 1;
        let Ok(leaf) = std::fs::symlink_metadata(&p) else {
            // Absent altogether: the CALLER stamps the directory whose mtime
            // moves when it appears. Costs the one probe `exists` cost.
            return;
        };
        if leaf.file_type().is_symlink() {
            // The leaf IS a link, so its owner's mtime stops governing here:
            // the TARGET can gain the store later without moving anything the
            // owner sees, and a link that dangles today is dropped below and
            // would never be witnessed at all. `witness` stamps the link AND
            // what it points at, target-absent included.
            self.witness(p.clone());
            self.probes += 1;
            if !p.exists() {
                return;
            }
        }
        self.probes += 1;
        let canonical = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        // A store reached through a symlink is only as stable as the link's
        // TARGET, and moving the target leaves the owning directory untouched.
        // Stamp the resolved path too, but only when it actually differs —
        // the ordinary real-directory case pays nothing.
        if canonical != p {
            self.witness(canonical.clone());
        }
        self.stores.push(canonical);
    }
}

fn scan_vcs_content_stores(root: &Path) -> StoreScan {
    let mut scan = StoreScan::new();
    // Whether ANY control directory exists at all is decided here: creating,
    // removing or re-pointing `<root>/.git` moves the root's own mtime.
    scan.witness(root.to_path_buf());
    // Whether a control directory gains, loses or re-points a store leaf is
    // settled by its own mtime; whether it EXISTS at all is settled by the
    // root's, stamped above. `scan_control_dirs_in` is what acts on that.
    scan.scan_control_dirs_in(root);
    scan
}

/// Workspace-relative content stores denied for reads in a secret-deny posture,
/// one per VCS. Each holds committed CONTENT; none is needed to answer a
/// metadata question (branch name, dirty state), which is what keeps the deny
/// from breaking ordinary session work.
///
/// Held as (control directory, store leaf) PAIRS rather than joined strings so
/// the same list drives two consumers that must not drift: the root-relative
/// join in [`vcs_content_stores`], and the any-depth shape test in
/// [`is_vcs_store_dir`] that #322 needs.
const VCS_CONTENT_STORES: &[(&str, &str)] = &[
    (".git", "objects"),
    (".git", "modules"),
    (".git", "lfs"),
    (".hg", "store"),
    (".svn", "pristine"),
    (".bzr", "repository"),
];

/// True when the last two components of `path` name a VCS content store — the
/// [`VCS_CONTENT_STORES`] shape recognised at ANY depth, not only directly
/// under the workspace root.
///
/// FerroxLabs/wayland-core#322: discovery was root-relative only
/// (`root.join(".git/objects")`), so a vendored or nested checkout
/// (`<root>/vendor/x/.git/objects`, a submodule working copy, a bundled example
/// repo) reconstructed a committed secret through its own porcelain while the
/// deny list covered nothing. Purely lexical over an already-canonicalized
/// path, so it costs no syscall.
///
/// Deliberately matches the STORE directory only. `<root>/vendor/x/.git/HEAD`
/// and `.../refs/heads/main` are not stores and stay readable, mirroring the
/// `git rev-parse` carve-out [`vcs_content_stores`] documents for the root
/// repository.
fn is_vcs_store_dir(path: &Path) -> bool {
    use std::path::Component;
    let mut rev = path.components().rev();
    let (Some(Component::Normal(leaf)), Some(Component::Normal(parent))) = (rev.next(), rev.next())
    else {
        return false;
    };
    VCS_CONTENT_STORES.iter().any(|(dir, store)| {
        parent == std::ffi::OsStr::new(dir) && leaf == std::ffi::OsStr::new(store)
    })
}

/// True when `path` IS a VCS content store or lives INSIDE one, or is the
/// control directory that owns one (`.git`, `.hg`, `.svn`, `.bzr`), at ANY
/// depth.
///
/// FerroxLabs/wayland-core#322 c4: the composer's `@dir` walk needs the same
/// any-depth treatment the deny walk gives, but it PRUNES rather than denies —
/// so it must stop at the control directory too, which is the only thing
/// standing between a tree walk and every object underneath. Both halves read
/// [`VCS_CONTENT_STORES`], so the walk and the deny list cannot drift apart.
///
/// The store half is [`inside_vcs_store`] — the SAME predicate the deny walk
/// asks through [`WorkspacePolicy::is_vcs_content_store`] — and not the
/// self-only [`is_vcs_store_dir`]. The two are NOT equivalent and the
/// difference is reachable: pruning governs only what the walk DESCENDS to,
/// while a symlink is an entry met at the top of the tree, so one aimed BELOW
/// a store root (`.git/objects/aa`, neither a `(control, store)` shape nor a
/// control-directory leaf) escaped the self-only test and every object under
/// it was inlined. Graded by
/// `at_dir_prunes_a_path_that_resolves_below_a_store_root`.
///
/// Purely lexical, like [`is_vcs_store_dir`]; the caller is responsible for
/// handing it an already-resolved path, which is what makes a store reached
/// under another name answer the same as one reached by its own.
pub fn is_within_vcs_store_or_control_dir(path: &Path) -> bool {
    if inside_vcs_store(path) {
        return true;
    }
    let Some(leaf) = path.file_name() else {
        return false;
    };
    VCS_CONTENT_STORES
        .iter()
        .any(|(dir, _)| leaf == std::ffi::OsStr::new(dir))
}

/// [`is_vcs_store_dir`] applied to `path` and every ancestor of it: true when
/// `path` IS a content store or lives inside one.
fn inside_vcs_store(path: &Path) -> bool {
    path.ancestors().any(is_vcs_store_dir)
}

/// True when `name` is one of the [`VCS_CONTENT_STORES`] STORE leaves
/// (`objects`, `modules`, `lfs`, `store`, `pristine`, `repository`),
/// irrespective of what its parent is called.
///
/// Read by arm 3 ([`WorkspacePolicy::encloses_repository_store`]) to decide
/// which ancestors are worth a repository probe at all. NOT a denial on its
/// own: every one of these is also an ordinary project directory name.
fn is_vcs_store_leaf_name(name: &std::ffi::OsStr) -> bool {
    VCS_CONTENT_STORES
        .iter()
        .any(|(_, store)| name == std::ffi::OsStr::new(store))
}

/// True when `name` is a VCS CONTROL directory name (`.git`, `.hg`, `.svn`,
/// `.bzr`), read off the same [`VCS_CONTENT_STORES`] table so the walk and the
/// deny list cannot drift.
fn is_vcs_control_dir_name(name: &std::ffi::OsStr) -> bool {
    VCS_CONTENT_STORES
        .iter()
        .any(|(control, _)| name == std::ffi::OsStr::new(control))
}

/// One arm-4 discovery walk: the stores every NESTED control directory (and
/// every nested bare repository) under `root` names, and what the walk cost.
struct NestedStoreScan {
    stores: Vec<PathBuf>,
    /// FerroxLabs/wayland-core#406 — the declaration sites this walk READ, with
    /// the mtime it saw (`None` for absent). See [`NestedStoreCache`].
    witnesses: Vec<(PathBuf, Option<SystemTime>)>,
    /// Set true for the declarations of `<root>/.git`, which arm 2 stamps.
    skip_witnesses: bool,
    probes: u64,
    stamped_at: SystemTime,
}

impl NestedStoreScan {
    /// Stamp a declaration site — a file whose CONTENT, or a store leaf whose
    /// TARGET, decided part of this walk's answer.
    ///
    /// Absent is a perfectly good stamp and is the one that matters here: it
    /// flips to `Some` the moment an `alternates` file is written into a
    /// control directory the walk already found, which is the #406 residual.
    fn witness(&mut self, path: PathBuf) {
        if self.skip_witnesses {
            return;
        }
        if self.witnesses.iter().any(|(seen, _)| *seen == path) {
            return;
        }
        self.probes += 1;
        let stamp = std::fs::symlink_metadata(&path)
            .and_then(|meta| meta.modified())
            .ok();
        self.witnesses.push((path, stamp));
    }

    /// Record a discovered store, canonicalized.
    ///
    /// Canonicalizing here rather than at query time is what makes the
    /// borrow-target NAME irrelevant (FerroxLabs/wayland-core#394): the set is
    /// tested by prefix against an already-resolved query path, so a store
    /// reached as `<root>/odb` and one reached as `<root>/vendor/pkg/.git/objects`
    /// through a symlink are the same entry.
    fn push_store(&mut self, path: PathBuf) {
        self.probes += 1;
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        if !self.stores.contains(&canonical) {
            self.stores.push(canonical);
        }
    }

    /// Every store a git control directory or gitdir at `dir` names, plus the
    /// stores it BORROWS.
    fn stores_named_by(&mut self, dir: &Path) {
        // #406 — ONE stamp for all six leaves, the same economy
        // `StoreScan::witness_if_present` already applies at the root: a leaf
        // cannot appear, vanish or be re-pointed without moving the mtime of
        // the directory that holds it, so stamping `dir` covers every store
        // this control directory could come to name. Six stats per checkout on
        // every admitted guard would be a revalidation as expensive as the
        // discovery it replaces.
        self.witness(dir.to_path_buf());
        for (_, leaf) in VCS_CONTENT_STORES {
            self.push_store(dir.join(leaf));
        }
        self.alternates_of(dir.join("objects"));
    }

    /// `objects/info/alternates` — one borrowed object database per line,
    /// absolute or relative to `objects_dir`, `#` comments skipped. Same format
    /// [`StoreScan::alternate_object_dirs`] reads for the root.
    fn alternates_of(&mut self, objects_dir: PathBuf) {
        // #406 — stamped whether or not it exists. An absent stamp is what
        // catches the borrow WRITTEN AFTER this walk into a control directory
        // the walk had already found.
        self.witness(objects_dir.join("info/alternates"));
        self.probes += 1;
        let Ok(text) = std::fs::read_to_string(objects_dir.join("info/alternates")) else {
            return;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let borrowed = resolve_against(&objects_dir, PathBuf::from(line));
            self.push_store(borrowed);
        }
    }
}

/// Arm 4 (FerroxLabs/wayland-core#390, #394, #396) — walk `root` ONCE and
/// resolve every content store that a control directory, a gitfile, a bare
/// repository or an `alternates` borrow nested under it NAMES.
///
/// Three things this walk does that the name-based arms could not:
///
/// * a VENDORED checkout's gitfile (`<root>/vendor/pkg/.git` = `gitdir: ../pkg-git`)
///   is read where it lies, so `<root>/vendor/pkg-git/objects` is denied even
///   though no component of that path is lexically a `(control, store)` pair
///   (#390 c1);
/// * borrow targets and symlinked store leaves are CANONICALIZED into the set,
///   so a store hidden behind a name like `odb` is covered by the same prefix
///   test as one called `objects` (#390 c2, #394 c1);
/// * a BARE repository is recognised by its own shape rather than by a control
///   directory it does not have (#396 c1).
///
/// **Fails closed.** A directory the walk cannot enumerate is recorded as a
/// store, so its contents are refused rather than silently unscanned; a
/// `continue` there would let an unreadable directory hide a checkout beneath
/// it. Reads under such a directory would fail at the OS anyway, so the
/// wrong-refusal cost is nil.
///
/// **Does not descend** into control directories (arm 1 already covers their
/// interiors lexically, at any depth), into content stores, or through
/// symlinked directories (a symlink is a jump to somewhere the walk either
/// reaches on its own or that arms 1-3 answer on the resolved path; following
/// them is how a walk finds a cycle).
fn discover_nested_content_stores(root: &Path) -> NestedStoreScan {
    let mut scan = NestedStoreScan {
        stores: Vec::new(),
        witnesses: Vec::new(),
        skip_witnesses: false,
        probes: 0,
        // Taken BEFORE any probe: anything modified from here on is inside the
        // walk's own window and must not be trusted by a revalidation.
        stamped_at: SystemTime::now(),
    };
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        // A bare repository (`git clone --bare|--mirror`, a submodule object
        // cache, a vendored mirror) has HEAD/refs/objects at its own top level
        // and no control directory anywhere. Its store leaves are recorded and
        // it is not descended into.
        scan.probes += 2;
        if std::fs::symlink_metadata(dir.join("HEAD")).is_ok()
            && (std::fs::symlink_metadata(dir.join("refs")).is_ok()
                || std::fs::symlink_metadata(dir.join("config")).is_ok())
        {
            scan.stores_named_by(&dir);
            continue;
        }
        scan.probes += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // Fail CLOSED: what cannot be enumerated cannot be cleared.
            scan.push_store(dir);
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                scan.push_store(dir.clone());
                continue;
            };
            let path = entry.path();
            let name = entry.file_name();
            scan.probes += 1;
            let Ok(file_type) = entry.file_type() else {
                scan.push_store(path);
                continue;
            };
            if is_vcs_control_dir_name(&name) {
                // #406 — `<root>/.git`'s own declarations are arm 2's job and
                // are already revalidated on every guard by
                // `vcs_store_cache_hit`. Stamping them here as well would add a
                // probe to the ordinary-path guard #398 c2 pins at three, and
                // buy no denial that arm 2 does not already make.
                scan.skip_witnesses = dir == root;
                if file_type.is_dir() {
                    // A store leaf inside it may be a SYMLINK out of the tree;
                    // `push_store` canonicalizes, so the target is what lands
                    // in the set.
                    scan.stores_named_by(&path);
                } else {
                    // A `.git` FILE (gitfile) or a `.git` SYMLINK to a real
                    // directory: both name a gitdir elsewhere. #242's shape,
                    // read at every depth rather than at the root only.
                    // #406 — the gitfile's own CONTENT names the gitdir, and
                    // a rewrite leaves every directory mtime untouched.
                    scan.witness(path.clone());
                    for gitdir in gitfile_targets(&path, &dir, &mut scan.probes) {
                        scan.witness(gitdir.join("commondir"));
                        scan.stores_named_by(&gitdir);
                    }
                }
                scan.skip_witnesses = false;
                continue;
            }
            if file_type.is_dir() && !is_vcs_store_dir(&path) {
                queue.push(path);
            }
        }
    }
    scan
}

/// The gitdir(s) a `.git` FILE names: its `gitdir:` line and, for a linked
/// worktree, the `commondir` that gitdir points at in turn.
///
/// Split out of [`StoreScan::gitfile_content_stores`]'s root-only form so the
/// arm-4 walk can read the same shape at any depth without carrying the
/// witness bookkeeping the memoised arm-2 scan needs.
fn gitfile_targets(gitfile: &Path, owner: &Path, probes: &mut u64) -> Vec<PathBuf> {
    *probes += 1;
    let Ok(text) = std::fs::read_to_string(gitfile) else {
        return Vec::new();
    };
    let Some(named) = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))
        .map(|rest| PathBuf::from(rest.trim()))
        .filter(|p| !p.as_os_str().is_empty())
    else {
        return Vec::new();
    };
    *probes += 1;
    let gitdir = resolve_against(owner, named);
    let mut dirs = vec![gitdir.clone()];
    *probes += 1;
    if let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) {
        let common = PathBuf::from(common.trim());
        if !common.as_os_str().is_empty() {
            dirs.push(resolve_against(&gitdir, common));
        }
    }
    dirs
}

/// #242 — content stores reached through a `.git` FILE rather than a `.git`
/// directory.
///
/// `git worktree add` writes `<worktree>/.git` as a one-line `gitdir: <path>`
/// pointer, so a workspace that IS a linked worktree has no `.git/objects` of
/// its own and the plain join in [`vcs_content_stores`] matches nothing — while
/// `git show HEAD:.env` inside it reads the main repository's object store
/// perfectly well. The gitdir it names may be absolute or relative to the
/// worktree, and for a linked worktree the objects live not in that gitdir but
/// in the COMMON dir it points at in turn (`<gitdir>/commondir`), so both are
/// resolved. `metadata` (not `symlink_metadata`) so a symlinked gitfile is
/// followed; a `.git` symlink to a real DIRECTORY needs nothing here, because
/// the `exists()` join above already follows it.
///
/// Denying a store outside every readable root is harmless — the child cannot
/// reach it either way — so this deliberately does not scope-check: the case
/// that matters is precisely the one where the external gitdir IS reachable.
impl StoreScan {
    fn gitfile_content_stores(&mut self, root: &Path) {
        let dot_git = root.join(".git");
        // Already stamped by the control-directory loop when it exists, which
        // is the only case that reaches past this line. A gitfile's CONTENT
        // decides everything below and a rewrite leaves the root's mtime
        // untouched, so that stamp is load-bearing here and not just for the
        // store leaves.
        debug_assert!(
            !dot_git.exists() || self.witnesses.iter().any(|(seen, _)| *seen == dot_git),
            "an existing `.git` must be stamped before its content is read"
        );
        self.probes += 1;
        if !std::fs::metadata(&dot_git).is_ok_and(|m| m.is_file()) {
            return;
        }
        self.probes += 1;
        let Ok(text) = std::fs::read_to_string(&dot_git) else {
            return;
        };
        let Some(named) = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("gitdir:"))
            .map(|rest| PathBuf::from(rest.trim()))
            .filter(|p| !p.as_os_str().is_empty())
        else {
            return;
        };
        self.probes += 1;
        let gitdir = resolve_against(root, named);
        let mut dirs = vec![gitdir.clone()];
        let commondir = gitdir.join("commondir");
        self.witness(commondir.clone());
        self.probes += 1;
        if let Ok(common) = std::fs::read_to_string(&commondir) {
            let common = PathBuf::from(common.trim());
            if !common.as_os_str().is_empty() {
                self.probes += 1;
                dirs.push(resolve_against(&gitdir, common));
            }
        }
        for dir in dirs {
            // Outside the root, so nothing already stamped moves if this
            // directory appears: it is stamped whether or not it exists.
            self.witness(dir.clone());
            for leaf in ["objects", "modules", "lfs"] {
                self.push_store(dir.join(leaf));
            }
            self.alternate_object_dirs(dir.join("objects"));
        }
    }
}

/// Object stores borrowed through `objects/info/alternates` — the third way a
/// git store lives outside `root` (after a gitfile and a symlink), and the one
/// `git clone --shared` / `--reference` produces. One path per line, absolute
/// or relative to `objects_dir`; a `#`-prefixed line is a comment.
impl StoreScan {
    fn alternate_object_dirs(&mut self, objects_dir: PathBuf) {
        // The alternates FILE is the witness, not its parents: its stamp flips
        // from absent to present the moment it is created at any depth below a
        // directory that does not exist yet, and its mtime moves when the
        // borrowed paths inside it change. One stamp, complete.
        let alternates = objects_dir.join("info/alternates");
        self.witness(alternates.clone());
        self.probes += 1;
        let Ok(text) = std::fs::read_to_string(&alternates) else {
            return;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            self.probes += 1;
            let borrowed = resolve_against(&objects_dir, PathBuf::from(line));
            if let Some(owner) = borrowed.parent() {
                self.witness(owner.to_path_buf());
            }
            self.push_store(borrowed);
        }
    }
}

// ===========================================================================
// RESOLVER INVENTORY -- FerroxLabs/wayland-core#402
//
// Every function in THIS FILE whose return type MENTIONS `PathBuf` and is not
// a collection of paths has exactly one `INVENTORY:` row below, classified
// with the reason. That is a structural test, not a list of spellings: the
// first version of this gate accepted four literal spellings and was blind to
// `std::io::Result<PathBuf>` -- the shape `std::fs::canonicalize` returns and
// one this file already uses -- and to a path carried inside a tuple. Both let
// a third resolver through in silence, which is the very defect below.
//
// It exists because core#356 c4's call-site gate is keyed to two literal
// resolver names, so a THIRD path resolver arrived ungated: its call sites
// need not say which resolver they use or why, and both name-keyed gates stay
// green, because a gate cannot notice a name it was not given. This block is
// the inverted question -- what does the file DEFINE -- and
// `tests::resolver_inventory_covers_every_pathbuf_returning_fn` fails when
// the block and the file disagree in EITHER direction.
//
// The classification, so a new row is a decision and not a guess:
//
// * `resolver` -- answers "where does a path supplied from OUTSIDE this file
//   land", for a path the caller may spell any way it likes, and this file
//   holds more than one answer. The caller therefore has a CHOICE, and
//   core#356 c4 obliges every call site to state which one it made. The gate
//   enforces that per row: a row marked `resolver` has its own call sites
//   checked for a ``resolver: `<name>``` note, so a third resolver is one row
//   and not a fourth hand-written test.
// * `helper` -- a private step of one resolver, a join of a path this file
//   already owns, or a constructor of a fixed location. No choice is offered
//   at its call sites, so no call-site note is owed -- and the reason it is
//   owed nothing is on the row rather than in a reviewer's head.
//
// INVENTORY: dir_stamp = helper: it resolves nothing. The path it returns is
//   the one the directory walk already produced -- it neither canonicalizes,
//   joins, nor re-attaches anything -- and its only job is to read that
//   directory's mtime with the SAME instrument revalidation uses. No caller is
//   offered a choice, so no call-site note is owed. It was invisible to the
//   first gate because its path rides inside `Option<(PathBuf, SystemTime)>`,
//   and a whitelist of four return-type spellings could not see a tuple.
//
// INVENTORY: canon_for_scope = resolver: the WEAK one. Canonicalizes what
//   exists and re-attaches one missing leaf, so it answers where a path's
//   SPELLING sits. Correct for advisory mirrors and $HOME lookups, wrong for
//   a refusal.
// INVENTORY: canon_existing_ancestor = resolver: the STRONG one. Walks the
//   dangling tail one symlink hop at a time, so it answers where a write or a
//   read would LAND. The resolver every `SecretDenyFs` guard predicate must
//   use (core#383 c3).
// INVENTORY: resolve = helper: the policy's counted wrapper over
//   `canon_existing_ancestor` for the two guard predicates. It IS one
//   resolver choice, already stated at its own body with the core#356 c4
//   note; a caller of `resolve` has no second answer to pick from.
// INVENTORY: resolve_against = helper: a private step of the VCS-declaration
//   scan. It joins a name read out of a gitfile / commondir / alternates
//   against that file's own directory, which git's own semantics fix; the
//   caller chooses nothing.
// INVENTORY: resolve_prefix = helper: the symlink-hop walk inside
//   `canon_existing_ancestor`, called from nowhere else.
// INVENTORY: canon_ancestor_only = helper: the walk-UP-and-append-verbatim
//   shape core#1097 abandoned as a resolver. It follows no symlink, so
//   `resolve_prefix` can call it without recursing into the hop walk. It must
//   never be reached from a predicate, which is why it is not a resolver row.
// INVENTORY: lexical_normalize = helper: applies `.` and `..` textually and
//   touches no filesystem. It keeps an unresolvable symlink target honest for
//   `resolve_prefix`; it resolves nothing.
// INVENTORY: canon = helper: `canonicalize().unwrap_or(p)`, with no
//   missing-component handling and no link hop. Used only for roots that
//   already exist at construction time, never for a caller-supplied path.
// INVENTORY: vcs_store_entry = helper: the per-entry decision of the
//   secret-deny WALK, not of a caller-supplied path. It is handed an entry the
//   walk already produced and returns that entry's own canonical path or
//   nothing; there is no spelling to resolve, because the walk supplied it.
// INVENTORY: secret_entry = helper: the sibling of `vcs_store_entry`, same
//   shape and same reason -- shared verbatim by the walk's serial and parallel
//   arms so the two cannot answer differently.
// INVENTORY: session_output_root = helper: constructs a FIXED location under
//   the root (`dunce::simplified(root).join(SESSION_OUTPUT_ROOT)`). A pure
//   string operation on a path this file already owns.
// INVENTORY: scratch_dir = helper: constructs a FIXED location under the host
//   temp directory from the trust level. No caller-supplied path reaches it.
// INVENTORY: auto_run_overlap = helper: SELECTS one of the auto-run locations
//   this file already owns and returns it verbatim. Lexical, and deliberately
//   resolves nothing -- an auto-run directory must be refused whether or not
//   it currently exists.
// INVENTORY: revoke_session_read_root = helper: returns the root of a grant
//   this policy already holds, resolved when the grant was made. A lookup, not
//   a resolution.
// INVENTORY: grant_session_read_root = helper: the two-argument form of
//   `grant_session_read_root_full`; it forwards and resolves nothing itself.
// INVENTORY: grant_session_read_root_full = helper: takes the path
//   `grantable_read_root` has already resolved and records it. The resolution
//   is that function's, and is graded there.
// INVENTORY: grantable_read_root = helper: the capacity half. It takes the
//   path `grantable_read_root_shape` has already resolved and returns it
//   unchanged, so the caller cannot re-resolve it differently -- the same
//   single-resolution discipline `resolve` gives the guard.
// INVENTORY: grantable_read_root_shape = resolver: the THIRD answer, and the
//   one that proves this block is not a two-name gate rewritten. It resolves a
//   HOST-supplied folder with bare `std::fs::canonicalize`, which REFUSES a
//   path that does not exist. Correct for a grant -- you cannot open a folder
//   that is not there, and a grant over a path that may yet be created is a
//   grant over whatever later takes the name -- and wrong for a guard, which
//   must judge where a not-yet-created file WOULD land. Its call sites carry
//   the core#356 c4 note for that reason.
// ===========================================================================

/// Resolve a VCS-file-supplied path: absolute as written, relative against the
/// file's own directory, then canonicalized so it compares against the rest of
/// the deny set on equal terms.
fn resolve_against(base: &Path, named: PathBuf) -> PathBuf {
    let joined = if named.is_absolute() {
        named
    } else {
        base.join(named)
    };
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Best-effort canonicalization for the under-root scope check. Falls back to
/// canonicalizing the parent + re-attaching the final component when `path`
/// itself does not exist (e.g. a `Write` to a not-yet-created `.env`), so the
/// `/var` → `/private/var` normalization still lands and the prefix match
/// against the canonical root holds.
pub(crate) fn canon_for_scope(path: &Path) -> PathBuf {
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

/// True when any ancestor of `path` is a `.wayland-core/skills` or
/// `.wayland-core/commands` directory.
///
/// Walks components rather than joining a root, because
/// `wcore_skills::paths::project_skills_dirs()` walks UP from the cwd: a
/// `.wayland-core/skills` in an ancestor of the workspace is a load path for
/// this session, and one in a sibling checkout is a load path for that one. A
/// component walk covers all of them with no root to be wrong about.
fn under_project_load_path(path: &Path) -> bool {
    use std::path::Component;
    let mut parent_was_marker = false;
    for component in path.components() {
        let Component::Normal(name) = component else {
            parent_was_marker = false;
            continue;
        };
        if parent_was_marker
            && wcore_config::config::SKILL_SOURCE_DIR_NAMES
                .iter()
                .any(|leaf| name == std::ffi::OsStr::new(leaf))
        {
            return true;
        }
        parent_was_marker = name == std::ffi::OsStr::new(".wayland-core");
    }
    false
}

/// Resolve `path` to where it would ACTUALLY land, component by component,
/// without requiring any of it to exist yet.
///
/// [`canon_for_scope`] resolves at most one missing component, which is enough
/// for a leaf that does not exist yet but not for a target whose directories
/// have not been created either (`<root>/.wayland-out/results/x.txt` on a fresh
/// workspace, which is what every FIRST spill looks like).
///
/// Walking DOWN and re-canonicalizing after every component — rather than
/// canonicalizing the longest existing ancestor once and appending the rest
/// verbatim — is what keeps the result honest for the two escapes that matter,
/// both of which appear only when part of the path is missing:
///
/// * `<root>/nope/../../outside/x` — a `..` that follows a component which
///   does not exist. Appended verbatim it stays in the string, and the result
///   still `starts_with` the root while the real target is outside it.
/// * `<root>/nope/../link/x` — a symlinked component reached only after such a
///   `..`. It has to be resolved before the prefix compare, not after.
///
/// A `..` is applied lexically (`pop`) because the prefix accumulated so far is
/// already canonical, so there is no symlink left for it to traverse wrongly.
fn canon_existing_ancestor(path: &Path) -> PathBuf {
    // FerroxLabs/wayland-core#376: an ABSOLUTE path that resolves in full needs
    // none of the walk below, and that is the overwhelmingly common case on the
    // per-operation guard path. `canonicalize` resolves every component and
    // every `..` in one syscall sequence, and it succeeds only when nothing is
    // missing and no link dangles -- which is precisely the condition under
    // which the component walk provably returns the same path. The walk exists
    // for the cases `canonicalize` REFUSES (a missing component, a dangling
    // link, a `..` after either), so this fast path can never cover one of them.
    //
    // Restricted to absolute paths on purpose: for a relative `../x` the walk
    // pops an EMPTY prefix and yields `x`, while `canonicalize` would resolve it
    // against the process cwd. Those disagree, and this is not the place to
    // change which one a caller gets.
    if path.is_absolute()
        && let Ok(resolved) = std::fs::canonicalize(path)
    {
        return resolved;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
        // Resolve as soon as the prefix so far exists, so a symlinked
        // component is replaced by its target BEFORE any later `..` is
        // applied to it, and so a `..` that follows a component which does
        // not exist is still applied instead of being carried into the
        // comparison verbatim.
        out = resolve_prefix(out);
    }
    out
}

/// A symlink chain is followed at most this many hops before the walk gives
/// up. Only a cycle reaches the bound; the write that follows it then fails
/// with the OS ELOOP of its own.
const MAX_SYMLINK_HOPS: usize = 16;

/// Resolve one accumulated prefix as far as the filesystem allows.
///
/// `std::fs::canonicalize` fails on a DANGLING symlink -- one whose target
/// does not exist yet. Leaving such a component verbatim makes the prefix
/// compare in [`WorkspacePolicy::ensure_write_target_readable`] judge where
/// the LINK sits instead of where the write would land, and `std::fs::write`
/// follows the link. Measured on hetzner-dsm before this change, with three
/// controls beside it: a dangling `<workspace>/out.txt -> <outside>/loot.txt`
/// was ACCEPTED and the bytes landed outside, while the same link with an
/// EXISTING target was refused. So a dangling link is followed by hand, one
/// hop at a time, and the result re-canonicalized.
fn resolve_prefix(mut out: PathBuf) -> PathBuf {
    for _ in 0..MAX_SYMLINK_HOPS {
        if let Ok(resolved) = std::fs::canonicalize(&out) {
            return resolved;
        }
        // Not a symlink (or gone entirely): an ordinary does-not-exist-yet
        // component. Its EXISTING ancestors still have to be canonicalized --
        // see `canon_ancestor_only`.
        let Ok(meta) = std::fs::symlink_metadata(&out) else {
            return canon_ancestor_only(out);
        };
        if !meta.file_type().is_symlink() {
            return canon_ancestor_only(out);
        }
        let Ok(target) = std::fs::read_link(&out) else {
            return canon_ancestor_only(out);
        };
        out = if target.is_absolute() {
            lexical_normalize(target)
        } else {
            let mut base = out;
            base.pop();
            base.push(target);
            lexical_normalize(base)
        };
    }
    canon_ancestor_only(out)
}

/// Canonicalize the deepest EXISTING ancestor of `path` and re-append the
/// components below it.
///
/// A dangling symlink's target is followed by hand in [`resolve_prefix`], and
/// the result of that walk is a path whose leaf does not exist -- so
/// `std::fs::canonicalize` cannot be applied to it as a whole. Returning it
/// verbatim was wrong: the readable root it is about to be compared against
/// went through `canonicalize`, and on any host where the workspace sits under
/// a symlinked directory the two spellings disagree.
///
/// macOS guarantees that disagreement, because `TMPDIR` lives under
/// `/var/folders` and `/var` is a symlink to `/private/var`. A dangling link
/// landing back INSIDE the workspace then compared as `/var/...` against a
/// root of `/private/var/...`, failed `starts_with`, and a legitimate write
/// was refused -- the control arm of
/// `a_dangling_symlink_out_of_the_workspace_is_refused`, on CI run
/// 32700730900. It is not macOS-only: any workspace reached through a symlink
/// hits it.
///
/// This does NOT follow symlinks itself -- it canonicalizes only the part that
/// already exists -- so it is safe to call from inside [`resolve_prefix`]
/// without recursing back into the hop walk.
fn canon_ancestor_only(path: PathBuf) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path.clone();
    loop {
        if let Ok(base) = std::fs::canonicalize(&cursor) {
            let mut resolved = base;
            for name in tail.iter().rev() {
                resolved.push(name);
            }
            return resolved;
        }
        let Some(name) = cursor.file_name() else {
            return path;
        };
        tail.push(name.to_os_string());
        if !cursor.pop() {
            return path;
        }
    }
}

/// Apply `.` and `..` textually. The caller re-canonicalizes wherever the
/// result exists, so this only has to keep an unresolvable symlink target
/// honest rather than be a full resolver.
fn lexical_normalize(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
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
