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
        let mut readable_canon: Vec<PathBuf> = std::iter::once(root.clone())
            .chain(writable_extra.iter().cloned())
            .chain(readable_extra.iter().cloned())
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .collect();
        readable_canon.sort();
        readable_canon.dedup();
        let secret_deny = compute_secret_deny(WorkspaceTrust::Trusted, &root, &readable_canon);

        Self {
            root,
            trust: WorkspaceTrust::Trusted,
            writable_extra,
            readable_extra,
            network: crate::bash::default_bash_network_policy(),
            cache_env: Vec::new(),
            secret_deny,
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
        let mut readable_canon: Vec<PathBuf> = std::iter::once(root.clone())
            .chain(writable_extra.iter().cloned())
            .chain(readable_extra.iter().cloned())
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .collect();
        readable_canon.sort();
        readable_canon.dedup();
        let secret_deny = compute_secret_deny(WorkspaceTrust::Contained, &root, &readable_canon);

        Self {
            root,
            trust: WorkspaceTrust::Contained,
            writable_extra,
            readable_extra,
            network: crate::bash::default_bash_network_policy(),
            cache_env,
            secret_deny,
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
    if trust == WorkspaceTrust::Contained {
        let walker = ignore::WalkBuilder::new(root)
            .standard_filters(false) // a .gitignore'd .env must still be denied
            .hidden(false)
            .follow_links(false)
            .build();
        for entry in walker.flatten() {
            let path = entry.path();
            let Ok(canon) = std::fs::canonicalize(path) else {
                continue;
            };
            // Direct secret files.
            if entry.file_type().is_some_and(|t| t.is_file())
                && is_secret_path_static(path)
                && under_mounted(&canon)
            {
                out.push(canon.clone());
            }
            // Symlink whose RESOLVED target is a secret → deny the link's
            // own (canonicalized) path so the read-through is masked.
            // External-target residual (target not under a mounted root) is
            // documented in the plan — backstopped by network-Deny.
            if entry.path_is_symlink() && is_secret_path_static(&canon) && under_mounted(&canon) {
                out.push(canon);
            }
        }
    }

    out.sort();
    out.dedup();
    out
}

fn canon(p: PathBuf) -> PathBuf {
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn scratch_dirs() -> Vec<PathBuf> {
    let tmp = std::env::temp_dir();
    vec![canon(tmp)]
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
    fn network_preserves_the_opt_in_default_deny() {
        // Default (no env) => Deny. The opt-in is honored elsewhere; here we
        // assert the policy does not hardcode Deny independent of the helper.
        let dir = tempfile::tempdir().unwrap();
        let p = WorkspacePolicy::contained(dir.path());
        assert_eq!(p.network(), crate::bash::default_bash_network_policy());
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
        // SAFETY: serial test; single-threaded env mutation.
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
}
