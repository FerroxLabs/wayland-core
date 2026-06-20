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
        let readable_extra = dirs::home_dir().into_iter().collect();
        Self {
            root,
            trust: WorkspaceTrust::Trusted,
            writable_extra,
            readable_extra,
            network: crate::bash::default_bash_network_policy(),
            cache_env: Vec::new(),
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
        Self {
            root,
            trust: WorkspaceTrust::Contained,
            writable_extra: scratch_dirs(),
            readable_extra,
            network: crate::bash::default_bash_network_policy(),
            cache_env,
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

    /// True if `path` is a secret that must stay denied even inside a
    /// writable root. Lexical; the VFS adapter calls this with the
    /// already-canonicalized path (see `SecretDenyFs`), so symlinks that
    /// resolve to a secret inside the root are caught.
    pub fn is_secret_path(&self, path: &Path) -> bool {
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
            // service-account*.json and *-key.json / *key.json
            if name.ends_with(".json")
                && (name.starts_with("service-account") || name.ends_with("key.json"))
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
}
