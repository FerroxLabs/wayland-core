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

use std::path::{Path, PathBuf};
use wcore_sandbox::manifest::NetworkPolicy;

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
    /// #667: this policy relies on the OS sandbox actually enforcing
    /// `fs_read_deny` to keep secrets unreadable from `Bash` — so `Bash` must be
    /// REFUSED when the active backend cannot enforce read-deny (else it fails
    /// open). True for `Contained` and for any `Trusted` policy that opted into
    /// project-secret denial (`with_project_secret_deny`, i.e. Full/remote). A
    /// genuinely-local `Trusted` session leaves it false and keeps its shell.
    secret_read_deny_required: bool,
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
            for sub in [".cache", ".cargo", ".npm", ".rustup"] {
                writable_extra.push(home.join(sub));
            }
        }
        let readable_extra: Vec<PathBuf> = dirs::home_dir().into_iter().collect();

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
            // Genuinely-local Trusted default: no project-secret denial, so the
            // Bash read-deny-enforcement gate does not apply. `with_project_secret_deny`
            // flips this to true for a Full/remote session (#667).
            secret_read_deny_required: false,
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
            // Contained denies project secrets → Bash must be refused when the
            // backend can't enforce read-deny (else `cat .env` fails open).
            secret_read_deny_required: true,
        }
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
        v.extend(self.writable_extra.iter().cloned());
        v
    }
    pub fn readable_roots(&self) -> Vec<PathBuf> {
        let mut v = self.writable_roots();
        v.extend(self.readable_extra.iter().cloned());
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
        if !self.secret_read_deny_required {
            return self.secret_deny.clone();
        }
        let readable_canon =
            readable_canon_roots(&self.root, &self.writable_extra, &self.readable_extra);
        let mut out = self.secret_deny.clone();
        out.extend(project_committed_secrets(&self.root, &readable_canon));
        out.extend(git_content_stores(&self.root));
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

/// Minimal read/exec toolchain dirs for a contained shell to run compilers.
fn minimal_toolchain_read_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for sub in [".rustup", ".cargo/bin"] {
            let p = home.join(sub);
            if p.exists() {
                v.push(p);
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn trusted_local_sets_cwd_and_does_not_redirect_caches() {
        let dir = tempfile::tempdir().unwrap();
        let p = WorkspacePolicy::trusted_local(dir.path());
        assert_eq!(p.trust(), WorkspaceTrust::Trusted);
        assert!(p.writable_roots().iter().any(|w| w == p.root()));
        // Root identity: writable_roots()[0] must equal the canonicalized tmpdir.
        assert_eq!(
            p.root(),
            std::fs::canonicalize(dir.path())
                .unwrap_or_else(|_| dir.path().to_path_buf())
                .as_path()
        );
        // Trusted reuses the user's global caches — no redirect.
        assert!(p.cache_env().is_empty());
    }

    #[test]
    fn contained_redirects_caches_into_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let p = WorkspacePolicy::contained(dir.path());
        assert_eq!(p.trust(), WorkspaceTrust::Contained);
        let cargo = p
            .cache_env()
            .iter()
            .find(|(k, _)| k == "CARGO_HOME")
            .expect("Contained redirects CARGO_HOME");
        assert!(Path::new(&cargo.1).starts_with(p.root()));
        assert!(p.cache_env().iter().any(|(k, _)| k == "npm_config_cache"));
        assert!(p.cache_env().iter().any(|(k, _)| k == "PIP_CACHE_DIR"));
    }

    #[test]
    fn network_is_gated_on_trust_posture() {
        // #657 (Overwatch ruling, Sean-confirmed): the bare `trusted_local`
        // constructor is fail-safe — it seeds network from the shared helper
        // (Deny unless `WAYLAND_BASH_ALLOW_NETWORK`), NOT unconditional Inherit.
        // Egress is granted only at bootstrap for a genuinely-local session; see
        // `local_bash_network` + `with_network`. Contained stays denied too.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            WorkspacePolicy::trusted_local(dir.path()).network(),
            crate::bash::default_bash_network_policy(),
            "bare trusted_local must be fail-safe (Deny default), not network-on"
        );
        assert_eq!(
            WorkspacePolicy::contained(dir.path()).network(),
            crate::bash::default_bash_network_policy(),
            "a contained workspace stays denied (env opt-in via the helper)"
        );
        // `with_network` is the explicit local grant applied at bootstrap.
        assert_eq!(
            WorkspacePolicy::trusted_local(dir.path())
                .with_network(NetworkPolicy::Inherit)
                .network(),
            NetworkPolicy::Inherit,
            "with_network must override the fail-safe default"
        );
    }

    #[test]
    fn local_bash_network_grants_inherit_only_without_channel_posture() {
        // The gate: a genuinely-local session (no channel posture) gets network
        // egress; any channel-attached session — including Full — stays on the
        // pre-#657 lockdown (default_bash_network_policy = Deny + env hatch).
        assert_eq!(
            local_bash_network(false),
            NetworkPolicy::Inherit,
            "genuinely-local session must get network egress"
        );
        assert_eq!(
            local_bash_network(true),
            crate::bash::default_bash_network_policy(),
            "a channel-attached session (incl Full) must stay on the Deny default"
        );
    }

    #[test]
    fn is_secret_path_flags_project_and_key_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = WorkspacePolicy::contained(root);
        for rel in [
            ".env",
            ".env.local",
            ".env.production",
            ".git/config",
            ".git/hooks/pre-commit",
            ".git-credentials",
            "deploy/key.pem",
            "server.key",
            "cert.p12",
            "cert.pfx",
            ".npmrc",
            ".netrc",
            ".aws/credentials",
            "terraform.tfstate",
            "terraform.tfstate.backup",
            "gradle.properties",
            "service-account.json",
            "ci-key.json",
            "keys/id_rsa",
            "id_ed25519",
            ".ssh/id_ecdsa",
        ] {
            assert!(p.is_secret_path(&root.join(rel)), "{rel} must be secret");
        }
    }

    #[test]
    fn is_secret_path_allows_ordinary_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = WorkspacePolicy::contained(root);
        for rel in [
            "src/main.rs",
            "README.md",
            "Cargo.toml",
            ".gitignore",
            "environment.rs",
            "package.json",
            "config.json",
        ] {
            assert!(
                !p.is_secret_path(&root.join(rel)),
                "{rel} must NOT be secret"
            );
        }
    }

    #[test]
    fn is_secret_path_does_not_overmatch_json() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = WorkspacePolicy::contained(root);

        // These must NOT be flagged — they share a suffix but are not credentials.
        for not_secret in ["monkey.json", "package.json", "config.json"] {
            assert!(
                !p.is_secret_path(&root.join(not_secret)),
                "{not_secret} must NOT be secret"
            );
        }

        // These MUST be flagged — bounded credential patterns.
        for secret in [
            "service-account.json",
            "service-account-prod.json",
            "ci-key.json",
            "app_key.json",
            "key.json",
        ] {
            assert!(
                p.is_secret_path(&root.join(secret)),
                "{secret} must be secret"
            );
        }
    }

    // ── Task 6 tests ──────────────────────────────────────────────────────────

    /// Contained mode: a project `.env` under the workspace root is in the
    /// deny list; `src/main.rs` is NOT.
    #[test]
    fn contained_includes_project_env_excludes_main_rs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Create the file so canonicalize succeeds.
        std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();

        let p = WorkspacePolicy::contained(root);
        let deny = p.secret_deny_paths();

        let env_canon = std::fs::canonicalize(root.join(".env")).unwrap();
        assert!(
            deny.contains(&env_canon),
            ".env must be in deny list; deny={deny:?}"
        );

        let main_canon = std::fs::canonicalize(root.join("src/main.rs")).unwrap();
        assert!(
            !deny.contains(&main_canon),
            "src/main.rs must NOT be in deny list"
        );
    }

    /// Trusted mode: the project `.env` under the workspace root is NOT in
    /// the deny list (Trusted only denies credential stores, not project files).
    #[test]
    fn trusted_excludes_project_env() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), b"SECRET=x").unwrap();

        let p = WorkspacePolicy::trusted_local(root);
        let deny = p.secret_deny_paths();

        let env_canon = std::fs::canonicalize(root.join(".env")).unwrap();
        assert!(
            !deny.contains(&env_canon),
            "Trusted must NOT deny project .env; deny={deny:?}"
        );
    }

    /// The active profile's OWN credential + OAuth stores (the Task 0.1 vault /
    /// plaintext fallback / OAuth tokens) are denied so an LLM-driven bash
    /// command cannot read them out of a Trusted sandbox, even though the
    /// profile home sits inside a mounted root.
    #[test]
    #[serial_test::serial]
    fn trusted_denies_wayland_profile_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Profile home inside the (readable) workspace root → mounted.
        let wh = root.join("profile-home");
        std::fs::create_dir_all(wh.join("oauth")).unwrap();
        std::fs::write(wh.join("credentials.toml"), b"secrets = {}").unwrap();
        std::fs::write(wh.join("oauth/chatgpt.json"), b"{}").unwrap();

        let prev = std::env::var_os("WAYLAND_HOME");
        // SAFETY: `#[serial_test::serial]` serializes every env-mutating test
        // in this binary, so this mutation cannot race another.
        unsafe { std::env::set_var("WAYLAND_HOME", &wh) };
        let p = WorkspacePolicy::trusted_local(root);
        // SAFETY: serial test; restore prior value (deny is already computed).
        match &prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
        let deny = p.secret_deny_paths();

        let cred = std::fs::canonicalize(wh.join("credentials.toml")).unwrap();
        assert!(
            deny.contains(&cred),
            "profile credentials.toml must be denied; deny={deny:?}"
        );
        let oauth = std::fs::canonicalize(wh.join("oauth")).unwrap();
        assert!(
            deny.contains(&oauth),
            "profile oauth dir must be denied; deny={deny:?}"
        );
    }

    /// Every emitted path is absolute.
    #[test]
    fn every_deny_path_is_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let p = WorkspacePolicy::contained(dir.path());
        for path in p.secret_deny_paths() {
            assert!(path.is_absolute(), "deny path must be absolute: {path:?}");
        }
    }

    /// Symlink `notes.txt -> .env` (both inside the workspace) causes
    /// `notes.txt`'s canonicalized path (= `.env`) to be denied in Contained
    /// mode. Because `fs::canonicalize` resolves the symlink, the canonical
    /// path equals `.env`'s canonical path and both end up in the deny list
    /// (deduped to one entry).
    #[cfg(unix)]
    #[test]
    fn contained_symlink_to_env_is_denied() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
        std::os::unix::fs::symlink(".env", root.join("notes.txt")).unwrap();

        let p = WorkspacePolicy::contained(root);
        let deny = p.secret_deny_paths();

        // canonicalize(notes.txt) resolves to canonicalize(.env)
        let env_canon = std::fs::canonicalize(root.join(".env")).unwrap();
        assert!(
            deny.contains(&env_canon),
            "symlink target (.env canonical) must be in deny list; deny={deny:?}"
        );
    }

    // ── #667: project-secret deny for Full-posture channel/remote ─────────────

    /// #667 Bash vector: `with_project_secret_deny` adds the project `.env` to
    /// `secret_deny_paths()` (which `bash.rs` feeds to the OS sandbox's
    /// `fs_read_deny`), matching Contained — while a bare `trusted_local` (the
    /// genuinely-local keyboard session) still does NOT (see
    /// `trusted_excludes_project_env`).
    #[test]
    fn with_project_secret_deny_denies_project_env_for_bash() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), b"SECRET=x").unwrap();

        let env_canon = std::fs::canonicalize(root.join(".env")).unwrap();

        let local = WorkspacePolicy::trusted_local(root);
        assert!(
            !local.secret_deny_paths().contains(&env_canon),
            "local keyboard session must stay EXEMPT (may read own .env)"
        );

        let remote = WorkspacePolicy::trusted_local(root).with_project_secret_deny();
        assert!(
            remote.secret_deny_paths().contains(&env_canon),
            "Full/remote session must deny project .env; deny={:?}",
            remote.secret_deny_paths()
        );
    }

    /// #667 read-path predicate: `is_project_secret` is TRUE for a secret-named
    /// file UNDER the workspace root and FALSE for both an ordinary in-root file
    /// and a secret-named file OUTSIDE the root (host secrets stay readable — a
    /// `Full` session is the trusted-remote-operator escape hatch).
    #[test]
    fn is_project_secret_is_scoped_to_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
        std::fs::write(root.join("main.rs"), b"fn main() {}").unwrap();

        // A secret sibling OUTSIDE the workspace root.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join(".env"), b"HOST=y").unwrap();

        let p = WorkspacePolicy::trusted_local(root);

        assert!(
            p.is_project_secret(&root.join(".env")),
            "in-root .env is a project secret"
        );
        assert!(
            !p.is_project_secret(&root.join("main.rs")),
            "ordinary in-root file is not a secret"
        );
        assert!(
            !p.is_project_secret(&outside.path().join(".env")),
            "a secret OUTSIDE the workspace root is out of scope (host secret)"
        );
    }

    /// #667: `is_project_secret` catches a project `.env` even when it did not
    /// exist at construction time (lexical name-match, no TOCTOU gap), and the
    /// under-root scope still resolves for a not-yet-created target (the
    /// `canon_for_scope` parent fallback normalizes `/var`→`/private/var`).
    #[test]
    fn is_project_secret_has_no_toctou_gap() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Policy built BEFORE the .env exists.
        let p = WorkspacePolicy::trusted_local(root);
        let late_env = root.join("config").join(".env");
        std::fs::create_dir_all(root.join("config")).unwrap();
        assert!(
            p.is_project_secret(&late_env),
            "a project secret created after construction must still be denied"
        );
    }

    /// #667: `with_project_secret_deny` is idempotent — applying it twice does
    /// not duplicate entries.
    #[test]
    fn with_project_secret_deny_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), b"SECRET=x").unwrap();

        let once = WorkspacePolicy::trusted_local(root)
            .with_project_secret_deny()
            .secret_deny_paths()
            .to_vec();
        let twice = WorkspacePolicy::trusted_local(root)
            .with_project_secret_deny()
            .with_project_secret_deny()
            .secret_deny_paths()
            .to_vec();
        assert_eq!(once, twice, "double-apply must not duplicate deny entries");
    }

    /// #667 F2: the `secret_read_deny_required` flag (which gates whether
    /// `bash.rs` refuses the shell on a non-enforcing backend) is set for
    /// Contained AND for a Full/remote `with_project_secret_deny` policy, but
    /// NOT for a bare local `trusted_local` — so a genuinely-local session keeps
    /// its shell while a remote one is fenced.
    #[test]
    fn secret_read_deny_required_tracks_project_secret_denial() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(
            !WorkspacePolicy::trusted_local(root).secret_read_deny_required(),
            "local Trusted must NOT require read-deny enforcement (keeps shell)"
        );
        assert!(
            WorkspacePolicy::trusted_local(root)
                .with_project_secret_deny()
                .secret_read_deny_required(),
            "Full/remote Trusted must require read-deny enforcement (#667 F2)"
        );
        assert!(
            WorkspacePolicy::contained(root).secret_read_deny_required(),
            "Contained must require read-deny enforcement"
        );
    }

    // ── #234: Bash OS-deny recomputed per-exec (post-bootstrap TOCTOU) ─────────

    /// #234 core: a Full/remote policy denies a secret CREATED AFTER
    /// construction via `secret_deny_paths_dynamic()`, while the frozen
    /// `secret_deny_paths()` (what `bash.rs` used before) MISSES it — proving the
    /// dynamic re-walk is what closes the `Bash cat terraform.tfstate` gap.
    #[test]
    fn dynamic_deny_catches_post_bootstrap_secret_remote() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Full/remote posture: project-secret denial active at construction,
        // but no secrets exist yet.
        let p = WorkspacePolicy::trusted_local(root).with_project_secret_deny();

        // Secrets appear AFTER bootstrap — the TOCTOU window.
        std::fs::write(root.join("deploy.pem"), b"-----BEGIN KEY-----").unwrap();
        std::fs::write(root.join("terraform.tfstate"), b"{}").unwrap();
        let pem = std::fs::canonicalize(root.join("deploy.pem")).unwrap();
        let tf = std::fs::canonicalize(root.join("terraform.tfstate")).unwrap();

        assert!(
            !p.secret_deny_paths().contains(&pem) && !p.secret_deny_paths().contains(&tf),
            "frozen list must MISS post-bootstrap secrets (that is the #234 gap)"
        );
        let dynamic = p.secret_deny_paths_dynamic();
        assert!(
            dynamic.contains(&pem),
            "dynamic deny must include the post-bootstrap *.pem; got {dynamic:?}"
        );
        assert!(
            dynamic.contains(&tf),
            "dynamic deny must include the post-bootstrap terraform.tfstate; got {dynamic:?}"
        );
    }

    /// #234 anti-bypass: the dynamic walk must NOT honor `.gitignore` — secrets
    /// are routinely gitignored, so an ignore-respecting walk would skip exactly
    /// what must be denied. (Inherited from `project_committed_secrets`, pinned
    /// here for the dynamic path.)
    #[test]
    fn dynamic_deny_ignores_gitignore_for_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), b"*.pem\n").unwrap();
        let p = WorkspacePolicy::trusted_local(root).with_project_secret_deny();
        std::fs::write(root.join("id.pem"), b"k").unwrap();
        let pem = std::fs::canonicalize(root.join("id.pem")).unwrap();
        assert!(
            p.secret_deny_paths_dynamic().contains(&pem),
            "a gitignored secret must STILL be denied (honoring .gitignore is the bypass)"
        );
    }

    /// #234 exemption preserved: a bare local-keyboard session (Trusted, no
    /// project-secret denial) gets NO dynamic walk — `secret_deny_paths_dynamic`
    /// equals the frozen list and the operator may still read their own `.env`.
    #[test]
    fn dynamic_deny_local_keyboard_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = WorkspacePolicy::trusted_local(root);
        std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
        assert_eq!(
            p.secret_deny_paths_dynamic(),
            p.secret_deny_paths().to_vec(),
            "local keyboard session stays exempt — no dynamic walk"
        );
        let env = std::fs::canonicalize(root.join(".env")).unwrap();
        assert!(
            !p.secret_deny_paths_dynamic().contains(&env),
            "local .env must remain readable for the genuinely-local operator"
        );
    }

    /// #234: the Contained posture also picks up a post-bootstrap secret through
    /// the dynamic re-walk.
    #[test]
    fn dynamic_deny_catches_post_bootstrap_secret_contained() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = WorkspacePolicy::contained(root);
        std::fs::write(root.join("secrets.pem"), b"k").unwrap();
        let pem = std::fs::canonicalize(root.join("secrets.pem")).unwrap();
        assert!(
            p.secret_deny_paths_dynamic().contains(&pem),
            "Contained must dynamically deny a post-bootstrap secret; got {:?}",
            p.secret_deny_paths_dynamic()
        );
    }

    /// MF3 (auditor) — the walk must NOT prune for detection: a secret INSIDE a
    /// `node_modules`/`target`/`.wcache` dir is denied to Bash just as the file
    /// tools' `is_project_secret` denies it, so `Bash cat node_modules/vendor/x.pem`
    /// cannot read what `Read` refuses. The earlier prune (my #234 DoS fix) opened
    /// exactly this hole; the fix is a lexical-first walk (no prune), not coverage
    /// dropping. Nested ordinary-dir secret still caught (control).
    #[test]
    fn dynamic_deny_covers_secret_inside_machine_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = WorkspacePolicy::trusted_local(root).with_project_secret_deny();

        // Committed secrets that happen to live under machine-named dirs — MUST
        // be denied (the file tools' predicate denies them, so Bash must too).
        for d in ["target", ".wcache", "node_modules"] {
            std::fs::create_dir_all(root.join(d).join("vendor")).unwrap();
            std::fs::write(root.join(d).join("vendor").join("x.pem"), b"k").unwrap();
        }
        // Ordinary nested secret (control).
        std::fs::create_dir_all(root.join("deploy").join("keys")).unwrap();
        std::fs::write(root.join("deploy").join("keys").join("prod.pem"), b"k").unwrap();

        let dynamic = p.secret_deny_paths_dynamic();
        for d in ["target", ".wcache", "node_modules"] {
            let secret = std::fs::canonicalize(root.join(d).join("vendor").join("x.pem")).unwrap();
            assert!(
                dynamic.contains(&secret),
                "a committed secret under {d}/ MUST be denied (no prune); got {dynamic:?}"
            );
        }
        let real =
            std::fs::canonicalize(root.join("deploy").join("keys").join("prod.pem")).unwrap();
        assert!(
            dynamic.contains(&real),
            "nested ordinary secret must be denied"
        );
    }

    /// MF4 (auditor) — the Contained posture must not under-deny secrets under
    /// machine-named dirs (the base branch denied them; the prune had regressed
    /// this). Restored by dropping the prune.
    #[test]
    fn contained_denies_secret_inside_machine_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("node_modules").join("v")).unwrap();
        std::fs::write(root.join("node_modules").join("v").join("id.pem"), b"k").unwrap();
        let p = WorkspacePolicy::contained(root);
        let secret =
            std::fs::canonicalize(root.join("node_modules").join("v").join("id.pem")).unwrap();
        // Both the frozen construction-time list and the dynamic list must cover it.
        assert!(
            p.secret_deny_paths().contains(&secret),
            "Contained construction-time deny must cover node_modules secret; got {:?}",
            p.secret_deny_paths()
        );
        assert!(
            p.secret_deny_paths_dynamic().contains(&secret),
            "Contained dynamic deny must cover node_modules secret; got {:?}",
            p.secret_deny_paths_dynamic()
        );
    }

    /// Auditor round-2 HIGH: the git object store must be in Bash's fs_read_deny
    /// for secret-deny postures so `git show HEAD:<committed secret>` cannot
    /// reconstruct it from `.git/objects`. Local keyboard stays exempt.
    #[test]
    fn dynamic_deny_covers_git_object_store() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git").join("objects")).unwrap();
        let objects = std::fs::canonicalize(root.join(".git").join("objects")).unwrap();

        let remote = WorkspacePolicy::trusted_local(root).with_project_secret_deny();
        assert!(
            remote.secret_deny_paths_dynamic().contains(&objects),
            "Full/remote Bash deny must include .git/objects (git-show leak); got {:?}",
            remote.secret_deny_paths_dynamic()
        );

        let local = WorkspacePolicy::trusted_local(root);
        assert!(
            !local.secret_deny_paths_dynamic().contains(&objects),
            "local keyboard must NOT newly-deny .git/objects (exempt)"
        );
    }

    /// #667 F3: a benign-named symlink whose target is a project secret is
    /// denied by `is_project_secret` even WITHOUT a `SandboxedFs` wrapper (the
    /// Full deployment) — because the predicate canonicalizes first. Guards the
    /// symlink read-through bypass on the Full read path.
    #[cfg(unix)]
    #[test]
    fn is_project_secret_resolves_symlink_to_secret() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join(".env"), b"SECRET=x").unwrap();
        std::os::unix::fs::symlink(root.join(".env"), root.join("notes.txt")).unwrap();

        let p = WorkspacePolicy::trusted_local(&root);
        assert!(
            p.is_project_secret(&root.join("notes.txt")),
            "a benign-named symlink to a project secret must be denied (canon-first)"
        );
    }
}
