//! Workspace filesystem host capability — `Deny*` default + `Gated*` impl.
//!
//! Path resolution: every path is resolved relative to `root` and rejected if
//! the canonicalised target escapes `root` (no `..` traversal).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use wcore_plugin_api::access_gate::PluginAccessGate;

pub trait WaylandHostWorkspace: Send + Sync {
    fn read(&self, path: &str) -> Result<Vec<u8>, String>;
    fn write(&self, path: &str, body: Vec<u8>) -> Result<(), String>;
}

/// Fail-closed workspace. Every read/write denied.
#[derive(Debug, Default)]
pub struct DenyHostWorkspace;

impl WaylandHostWorkspace for DenyHostWorkspace {
    fn read(&self, _path: &str) -> Result<Vec<u8>, String> {
        Err("permission denied: workspace".into())
    }
    fn write(&self, _path: &str, _body: Vec<u8>) -> Result<(), String> {
        Err("permission denied: workspace".into())
    }
}

/// Gated workspace host. Read/write rooted at `root`.
pub struct GatedHostWorkspace {
    #[allow(dead_code)]
    gate: Arc<PluginAccessGate>,
    #[allow(dead_code)]
    plugin: String,
    root: PathBuf,
    permitted_read: bool,
    permitted_write: bool,
}

impl GatedHostWorkspace {
    pub fn new(
        gate: Arc<PluginAccessGate>,
        plugin: String,
        root: PathBuf,
        permitted_read: bool,
        permitted_write: bool,
    ) -> Self {
        Self {
            gate,
            plugin,
            root,
            permitted_read,
            permitted_write,
        }
    }

    /// Resolve `path` against `root`, rejecting traversal AND symlink escapes.
    ///
    /// Lexical `..` rejection alone (the prior implementation) does not stop a
    /// symlink planted under `root` from pointing outside it (M-5/plugins-7).
    /// After the join we canonicalize the deepest existing ancestor of the
    /// resolved path and require the canonical result to stay within the
    /// canonical root — collapsing any symlink in the chain.
    fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let candidate = Path::new(path);
        if candidate.is_absolute() {
            return Err("workspace: absolute paths rejected".into());
        }
        for c in candidate.components() {
            if matches!(c, Component::ParentDir) {
                return Err("workspace: path traversal rejected".into());
            }
        }
        let resolved = self.root.join(candidate);

        // Canonicalize the root (it must already exist). If the root itself
        // can't be canonicalized, fail closed.
        let canon_root = self
            .root
            .canonicalize()
            .map_err(|e| format!("workspace: root canonicalize failed: {e}"))?;

        // The target may not exist yet (writes create it). Canonicalize the
        // deepest existing ancestor, then re-append the not-yet-existing tail,
        // so a symlink anywhere in the existing chain is resolved and checked.
        let canon_target = canonicalize_through_existing(&resolved)
            .map_err(|e| format!("workspace: path canonicalize failed: {e}"))?;

        if !canon_target.starts_with(&canon_root) {
            return Err("workspace: path escapes workspace root".into());
        }
        Ok(canon_target)
    }
}

/// Canonicalize the deepest existing ancestor of `path`, then re-join the
/// remaining (non-existent) components. This resolves every symlink in the
/// existing portion of the chain while still permitting paths that don't
/// exist yet (the write case).
///
/// A non-existent path component may still be a *dangling* symlink (the link
/// exists, but its target does not). `Path::exists()` follows the link and so
/// reports `false`, which would otherwise let the link be re-joined as an inert
/// lexical name — allowing the caller to write/read through it and escape the
/// root. We therefore lstat each component with `symlink_metadata`: any
/// component that is itself a symlink (dangling or not) is rejected so it can
/// never be treated as a safe lexical tail.
fn canonicalize_through_existing(path: &Path) -> std::io::Result<PathBuf> {
    // Walk up to the first existing ancestor.
    let mut existing = path;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        // A dangling symlink fails `exists()` but is caught by lstat; reject it
        // rather than letting it be re-joined as an inert lexical component.
        if let Ok(meta) = existing.symlink_metadata()
            && meta.file_type().is_symlink()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "symlink component rejected",
            ));
        }
        if existing.exists() {
            break;
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                existing = parent;
            }
            _ => break,
        }
    }
    let mut canon = existing.canonicalize()?;
    for name in tail.iter().rev() {
        canon.push(name);
    }
    Ok(canon)
}

impl WaylandHostWorkspace for GatedHostWorkspace {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        if !self.permitted_read {
            return Err("permission denied: workspace".into());
        }
        let resolved = self.resolve(path)?;
        std::fs::read(&resolved).map_err(|e| format!("workspace read failed: {e}"))
    }

    fn write(&self, path: &str, body: Vec<u8>) -> Result<(), String> {
        if !self.permitted_write {
            return Err("permission denied: workspace".into());
        }
        let resolved = self.resolve(path)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("workspace mkdir failed: {e}"))?;
        }
        std::fs::write(&resolved, &body).map_err(|e| format!("workspace write failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn denied_workspace_returns_err() {
        let d = DenyHostWorkspace;
        assert!(d.read("x").is_err());
        assert!(d.write("x", vec![1]).is_err());
    }

    #[test]
    fn gated_workspace_without_read_perm_denies() {
        let dir = tempdir().unwrap();
        let w = GatedHostWorkspace::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            dir.path().to_path_buf(),
            false,
            true,
        );
        assert!(w.read("x").is_err());
    }

    #[test]
    fn gated_workspace_round_trip() {
        let dir = tempdir().unwrap();
        let w = GatedHostWorkspace::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            dir.path().to_path_buf(),
            true,
            true,
        );
        w.write("a/b.txt", b"hi".to_vec()).unwrap();
        assert_eq!(w.read("a/b.txt").unwrap(), b"hi");
    }

    #[test]
    fn gated_workspace_rejects_traversal() {
        let dir = tempdir().unwrap();
        let w = GatedHostWorkspace::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            dir.path().to_path_buf(),
            true,
            true,
        );
        assert!(w.read("../escape").is_err());
        assert!(w.write("../escape", vec![1]).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn gated_workspace_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        // Lay out: root/ and a sibling secret dir OUTSIDE root.
        let base = tempdir().unwrap();
        let root = base.path().join("root");
        let outside = base.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"TOPSECRET").unwrap();

        // Plant a symlink UNDER root that points outside it.
        symlink(&outside, root.join("escape")).unwrap();

        let w = GatedHostWorkspace::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            root.clone(),
            true,
            true,
        );

        // Reading through the symlink must be rejected (no off-root read).
        let read_res = w.read("escape/secret.txt");
        assert!(
            read_res.is_err(),
            "symlink escape read should be rejected, got {read_res:?}"
        );
        assert!(
            read_res.unwrap_err().contains("escapes workspace root"),
            "wrong rejection reason"
        );

        // Writing through the symlink must be rejected (no off-root write).
        let write_res = w.write("escape/planted.txt", b"x".to_vec());
        assert!(
            write_res.is_err(),
            "symlink escape write should be rejected, got {write_res:?}"
        );
        // And nothing was written outside the root.
        assert!(!outside.join("planted.txt").exists());
    }

    #[test]
    #[cfg(unix)]
    fn gated_workspace_rejects_dangling_symlink_leaf_escape() {
        use std::os::unix::fs::symlink;

        // root/ and an outside dir that EXISTS, but the target leaf does NOT.
        let base = tempdir().unwrap();
        let root = base.path().join("root");
        let outside = base.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();

        // Plant a DANGLING symlink leaf under root: outside/ exists but
        // outside/newfile.txt does not, so `root/escape.txt`.exists() == false.
        let dangling_target = outside.join("newfile.txt");
        let link = root.join("escape.txt");
        symlink(&dangling_target, &link).unwrap();
        assert!(
            !link.exists(),
            "leaf must be a dangling symlink for this test"
        );

        let w = GatedHostWorkspace::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            root.clone(),
            true,
            true,
        );

        // Writing through the dangling symlink must be rejected — it must NOT
        // follow the link and create outside/newfile.txt.
        let write_res = w.write("escape.txt", b"pwned".to_vec());
        assert!(
            write_res.is_err(),
            "dangling-symlink-leaf write should be rejected, got {write_res:?}"
        );
        assert!(
            !dangling_target.exists(),
            "write escaped the workspace root via dangling symlink"
        );

        // Reading through the dangling symlink must also be rejected.
        let read_res = w.read("escape.txt");
        assert!(
            read_res.is_err(),
            "dangling-symlink-leaf read should be rejected, got {read_res:?}"
        );
    }

    #[test]
    fn gated_workspace_rejects_absolute() {
        let dir = tempdir().unwrap();
        let w = GatedHostWorkspace::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            dir.path().to_path_buf(),
            true,
            true,
        );
        assert!(w.read("/etc/passwd").is_err());
    }
}
