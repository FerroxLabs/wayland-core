//! W8a A.3 — VirtualFs trait + RealFs / InMemoryFs / SandboxedFs impls (X2).
//!
//! Tools that touch the filesystem go through `ToolContext.vfs` (an
//! `Arc<dyn VirtualFs>`) so the engine can swap RealFs for an in-memory
//! mock in tests, and clamp sub-agents to a `SandboxedFs { root }`
//! rooted at their workspace.
//!
//! Wave SD hardening (SECURITY MAJORs #13 + #14 + closed in tandem with
//! the legacy `execute()` validation in read.rs / write.rs / edit.rs):
//!
//! 1. `fallthrough_reads` is **gone**. Reads are sandbox-checked the
//!    same way writes are. The previous escape hatch let a sub-agent
//!    `Read("/etc/passwd")` whenever the host flipped the flag for
//!    performance. If a use case really needs broader reads, callers
//!    must build a `SandboxPolicy { read_allowlist, write_allowlist }`
//!    and pass paths through explicit allow-list checks.
//!
//! 2. `contain()` now resolves symlinks via `std::fs::canonicalize`
//!    BEFORE the containment compare. Lex-normalization (`..` collapse)
//!    is only used for paths that don't yet exist. A symlink planted
//!    inside the sandbox that points outside is detected and refused.
//!    TOCTOU: the canonicalize re-runs on every operation — never
//!    cached — so swapping the symlink between two ops doesn't escape.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VfsError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("path {path:?} is outside sandbox root {root:?}")]
    OutsideSandbox { path: PathBuf, root: PathBuf },
    #[error("path {path:?} not found")]
    NotFound { path: PathBuf },
    #[error("refused: {path:?} is a protected secret path")]
    SecretDenied { path: PathBuf },
}

/// Provider-neutral filesystem the agent runs against.
///
/// All methods take `&Path` and return `VfsError`. Implementors are
/// expected to be `Send + Sync` so they can be shared via `Arc`.
#[async_trait]
pub trait VirtualFs: Send + Sync {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError>;
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError>;
    async fn exists(&self, path: &Path) -> Result<bool, VfsError>;
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError>;
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError>;
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError>;

    /// The containment root for a sandboxed filesystem, or `None` for an
    /// unconstrained one (`RealFs`, `InMemoryFs`). Tools that shell out to a
    /// subprocess (e.g. Grep → `rg`/`grep`) can't route the scan through the
    /// vfs, so they use this to anchor the subprocess working directory to the
    /// jail root — making a relative search path resolve against the sandbox,
    /// not the process cwd (F36).
    fn root(&self) -> Option<&Path> {
        None
    }
}

/// Minimum metadata surface tools need (size + is_dir). Avoids leaking
/// `std::fs::Metadata` into the trait so InMemoryFs can be honest about
/// its lack of filesystem-grade attributes.
#[derive(Debug, Clone)]
pub struct VfsMetadata {
    pub size: u64,
    pub is_dir: bool,
}

/// RealFs — passes through to `tokio::fs`.
pub struct RealFs;

#[async_trait]
impl VirtualFs for RealFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        Ok(tokio::fs::read(path).await?)
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        let path_owned = path.to_path_buf();
        let data = contents.to_vec();
        tokio::task::spawn_blocking(move || wcore_config::atomic_write(&path_owned, &data))
            .await
            .map_err(|e| VfsError::Io(std::io::Error::other(e)))??;
        Ok(())
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        Ok(tokio::fs::try_exists(path).await?)
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        let mut out = Vec::new();
        while let Some(e) = entries.next_entry().await? {
            out.push(e.path());
        }
        Ok(out)
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        Ok(tokio::fs::remove_file(path).await?)
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        let m = tokio::fs::metadata(path).await?;
        Ok(VfsMetadata {
            size: m.len(),
            is_dir: m.is_dir(),
        })
    }
}

/// InMemoryFs — pure ephemeral byte store. Used in tests to isolate
/// tool tests from real disk.
#[derive(Default)]
pub struct InMemoryFs {
    files: Arc<RwLock<std::collections::HashMap<PathBuf, Vec<u8>>>>,
}

impl InMemoryFs {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl VirtualFs for InMemoryFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.files
            .read()
            .get(path)
            .cloned()
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_path_buf(),
            })
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        self.files
            .write()
            .insert(path.to_path_buf(), contents.to_vec());
        Ok(())
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        Ok(self.files.read().contains_key(path))
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        Ok(self
            .files
            .read()
            .keys()
            .filter(|p| p.parent() == Some(dir))
            .cloned()
            .collect())
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        self.files
            .write()
            .remove(path)
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_path_buf(),
            })?;
        Ok(())
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        let files = self.files.read();
        let bytes = files.get(path).ok_or_else(|| VfsError::NotFound {
            path: path.to_path_buf(),
        })?;
        Ok(VfsMetadata {
            size: bytes.len() as u64,
            is_dir: false,
        })
    }
}

/// SandboxedFs — wraps a `VirtualFs` (typically `RealFs`) and rejects
/// any operation whose canonical path escapes `root`. Reads and writes
/// both apply the same containment check; there is intentionally no
/// "fallthrough_reads" footgun (Wave SD SECURITY MAJOR #13).
pub struct SandboxedFs<F: VirtualFs> {
    inner: F,
    root: PathBuf,
}

impl<F: VirtualFs> SandboxedFs<F> {
    /// `root` is canonicalized on construction so the contain check
    /// compares apples to apples (e.g. macOS `/var` → `/private/var`).
    /// Falls back to `root` if canonicalization fails (dir doesn't
    /// exist yet); per-op containment still re-checks the live
    /// filesystem.
    pub fn new(inner: F, root: impl Into<PathBuf>) -> Self {
        let raw = root.into();
        let root = fs::canonicalize(&raw).unwrap_or(raw);
        Self { inner, root }
    }

    /// Returns Ok when `path` resolves inside `self.root`, Err
    /// otherwise.
    ///
    /// Strategy:
    ///   1. Lexically normalize the candidate path (strip `.`, collapse
    ///      `..`) — this rejects classic traversal strings before any
    ///      I/O.
    ///   2. Canonicalize the longest existing prefix via `fs::canonicalize`,
    ///      which **resolves symlinks**. The result MUST start with
    ///      `self.root` after the same canonicalization step that ran
    ///      in `new()`. This closes the SECURITY MAJOR #13 symlink
    ///      bypass: a symlink `<root>/escape -> /etc` lex-normalizes
    ///      to `<root>/escape` (in-bounds) but canonicalize() returns
    ///      `/etc` (out of bounds) and we refuse.
    ///   3. For paths whose existing prefix is exactly `self.root`
    ///      (i.e. the leaf doesn't exist yet — e.g. a write target),
    ///      step 2's canonical prefix already starts with `self.root`,
    ///      so the suffix is allowed because no symlink can escape
    ///      through a not-yet-created node.
    fn contain(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let normalized = lex_normalize(path, &self.root);

        // Walk up the path to the longest existing prefix, canonicalize
        // it (which follows symlinks), and check the canonical form
        // sits inside `self.root`. If the prefix canonicalizes to
        // somewhere outside the root, refuse — even if the trailing
        // not-yet-existing suffix is benign.
        let (canon_prefix, suffix) = match canonicalize_existing_prefix(&normalized) {
            Some((prefix, suffix)) => (prefix, suffix),
            None => {
                return Err(VfsError::OutsideSandbox {
                    path: normalized,
                    root: self.root.clone(),
                });
            }
        };

        if !canon_prefix.starts_with(&self.root) {
            return Err(VfsError::OutsideSandbox {
                path: normalized,
                root: self.root.clone(),
            });
        }

        // Re-assemble: canonical prefix + (still-relative) suffix.
        // When the entire path already exists `suffix` is empty and the
        // canonical prefix IS the read target; `PathBuf::join("")` would
        // leave a stray trailing separator on some platforms (turns a
        // file lookup into a dir lookup → ENOTDIR), so short-circuit.
        if suffix.as_os_str().is_empty() {
            Ok(canon_prefix)
        } else {
            Ok(canon_prefix.join(suffix))
        }
    }
}

/// Find the longest existing ancestor of `path` and return its
/// canonical form plus the (possibly empty) trailing not-yet-existing
/// suffix. Returns `None` only when even `path.ancestors()` can't yield
/// a real prefix (e.g. relative path with no anchor) — the caller
/// should refuse such inputs.
fn canonicalize_existing_prefix(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut p: &Path = path;
    loop {
        if let Ok(canon) = fs::canonicalize(p) {
            // Suffix is the part of `path` that lives beyond `p`. When
            // `p == path` (the whole path exists and canonicalized
            // cleanly), the suffix is empty and the read target IS the
            // canonical form — don't join `""` since some PathBuf
            // implementations append `/` and turn a file lookup into a
            // dir lookup ("Not a directory" / ENOTDIR).
            let suffix = path.strip_prefix(p).unwrap_or(Path::new(""));
            return Some((canon, suffix.to_path_buf()));
        }
        p = p.parent()?;
    }
}

fn lex_normalize(path: &Path, base: &Path) -> PathBuf {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for c in candidate.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(c.as_os_str());
            }
        }
    }
    out
}

#[async_trait]
impl<F: VirtualFs + 'static> VirtualFs for SandboxedFs<F> {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        let p = self.contain(path)?;
        self.inner.read(&p).await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        let p = self.contain(path)?;
        self.inner.write(&p, contents).await
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        let p = self.contain(path)?;
        self.inner.exists(&p).await
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        let p = self.contain(dir)?;
        self.inner.list(&p).await
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        let p = self.contain(path)?;
        self.inner.remove_file(&p).await
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        let p = self.contain(path)?;
        self.inner.metadata(&p).await
    }
    fn root(&self) -> Option<&Path> {
        Some(&self.root)
    }
}

/// Wraps a `VirtualFs` and refuses any op whose path is a secret per the
/// active `WorkspacePolicy`. Layer INSIDE `SandboxedFs`
/// (`SandboxedFs::new(SecretDenyFs::new(RealFs, p), root)`) so it inspects
/// the canonicalized path and catches symlinks-to-secrets inside the root.
pub struct SecretDenyFs<F: VirtualFs> {
    inner: F,
    policy: std::sync::Arc<crate::workspace_policy::WorkspacePolicy>,
}

impl<F: VirtualFs> SecretDenyFs<F> {
    pub fn new(inner: F, policy: std::sync::Arc<crate::workspace_policy::WorkspacePolicy>) -> Self {
        Self { inner, policy }
    }
    fn guard(&self, path: &Path) -> Result<(), VfsError> {
        if self.policy.is_secret_path(path) {
            return Err(VfsError::SecretDenied {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl<F: VirtualFs + 'static> VirtualFs for SecretDenyFs<F> {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        self.guard(path)?;
        self.inner.read(path).await
    }
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<(), VfsError> {
        self.guard(path)?;
        self.inner.write(path, contents).await
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        self.guard(path)?;
        self.inner.exists(path).await
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        self.guard(dir)?;
        self.inner.list(dir).await
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        self.guard(path)?;
        self.inner.remove_file(path).await
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        self.guard(path)?;
        self.inner.metadata(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn secret_deny_fs_blocks_and_passes() {
        use crate::workspace_policy::WorkspacePolicy;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".env"), b"TOKEN=abc").unwrap();
        let policy = Arc::new(WorkspacePolicy::contained(root));
        let fs = SecretDenyFs::new(RealFs, Arc::clone(&policy));

        assert!(matches!(
            fs.read(&root.join(".env")).await,
            Err(VfsError::SecretDenied { .. })
        ));
        assert!(matches!(
            fs.write(&root.join(".env"), b"x").await,
            Err(VfsError::SecretDenied { .. })
        ));

        fs.write(&root.join("main.rs"), b"fn main(){}")
            .await
            .unwrap();
        assert_eq!(
            fs.read(&root.join("main.rs")).await.unwrap(),
            b"fn main(){}"
        );
    }

    // Unix-only: exercises `std::os::unix::fs::symlink`. Gating the whole test
    // with `#[cfg(unix)]` (rather than an inner `#[cfg(not(unix))] return;`)
    // avoids an `unreachable_code` error on Windows under `-D warnings`.
    #[cfg(unix)]
    #[tokio::test]
    async fn secret_deny_catches_symlink_to_secret_when_inner() {
        // Load-bearing: SecretDenyFs must be layered INSIDE SandboxedFs so it
        // sees the canonical (symlink-resolved) path. A benign-named symlink
        // pointing at .env must be denied.
        use crate::workspace_policy::WorkspacePolicy;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join(".env"), b"TOKEN=abc").unwrap();
        std::os::unix::fs::symlink(root.join(".env"), root.join("notes.txt")).unwrap();

        let policy = Arc::new(WorkspacePolicy::contained(&root));
        let jail = SandboxedFs::new(SecretDenyFs::new(RealFs, Arc::clone(&policy)), root.clone());
        assert!(matches!(
            jail.read(&root.join("notes.txt")).await,
            Err(VfsError::SecretDenied { .. })
        ));
    }
}
