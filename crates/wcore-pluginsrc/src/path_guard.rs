//! Path guards for strings that arrive from a FOREIGN plugin manifest.
//!
//! The Claude Code adapter reads only fixed, adapter-chosen directories, so it
//! never joins an attacker-supplied string onto the plugin root. The Codex
//! format does: `plugin.json` declares `skills`, `commands`, `mcpServers`,
//! `hooks` and `apps` as relative path strings. Every one of those is
//! attacker-controlled, so every one has to pass through here before it is
//! joined, stat-ed, read or copied.
//!
//! Two layers, because one is not enough:
//!
//! * [`reject_traversal`] is lexical. It rejects absolute paths, `..`
//!   components, and Windows root/prefix components. Absolute paths matter
//!   because `Path::join` REPLACES its base when the argument is absolute —
//!   `root.join("/etc")` is `/etc`, and `root.join(r"C:\")` is `C:\`.
//! * [`resolve_within`] additionally resolves symlinks and confirms the result
//!   is still inside the root, because a symlinked intermediate directory
//!   inside the fetched tree can escape a path that is lexically clean.
//!
//! This is the single implementation for the whole workspace: `wcore-cli`'s
//! marketplace parser delegates to [`reject_traversal`] rather than carrying a
//! second copy of the rule.

use std::path::{Component, Path, PathBuf};

use crate::Result;
use crate::error::PluginSrcError;

/// Reject a path string that is absolute or contains a `..` / root / prefix
/// component. Returns [`PluginSrcError::PathTraversal`] naming the offending
/// string.
pub fn reject_traversal(s: &str) -> Result<()> {
    let p = Path::new(s);
    if p.is_absolute() {
        return Err(PluginSrcError::PathTraversal(s.to_string()));
    }
    let bad = p.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if bad {
        return Err(PluginSrcError::PathTraversal(s.to_string()));
    }
    Ok(())
}

/// Join a manifest-supplied relative path onto `root` and confirm containment.
///
/// Runs [`reject_traversal`] first, then — when the joined path actually exists
/// — resolves symlinks on both sides and requires the result to stay under
/// `root`. A path that does not exist is returned unresolved: the lexical check
/// already proved it cannot escape by `..`, and nothing reads a path that isn't
/// there. A path that exists but cannot be canonicalized (a dangling symlink)
/// is rejected rather than trusted.
pub fn resolve_within(root: &Path, rel: &str) -> Result<PathBuf> {
    reject_traversal(rel)?;
    let joined = root.join(rel.trim_start_matches("./"));
    if std::fs::symlink_metadata(&joined).is_err() {
        // Not present — nothing to resolve and nothing to read.
        return Ok(joined);
    }
    let root_canon = root
        .canonicalize()
        .map_err(|_| PluginSrcError::PathTraversal(rel.to_string()))?;
    let joined_canon = joined
        .canonicalize()
        .map_err(|_| PluginSrcError::PathTraversal(rel.to_string()))?;
    if !joined_canon.starts_with(&root_canon) {
        return Err(PluginSrcError::PathTraversal(rel.to_string()));
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn plain_relative_path_is_accepted() {
        assert!(reject_traversal("./skills/review").is_ok());
        assert!(reject_traversal("skills/review").is_ok());
    }

    #[test]
    fn parent_component_is_rejected() {
        let e = reject_traversal("../../etc/passwd").unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn embedded_parent_component_is_rejected() {
        // The `..` is not at the front — a prefix-only check would miss it.
        let e = reject_traversal("skills/../../../etc").unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn absolute_path_is_rejected() {
        #[cfg(unix)]
        let abs = "/etc/passwd";
        #[cfg(windows)]
        let abs = r"C:\Windows\System32";
        let e = reject_traversal(abs).unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[test]
    fn resolve_within_returns_joined_path_for_clean_input() {
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join("skills")).unwrap();
        let got = resolve_within(d.path(), "./skills").unwrap();
        assert_eq!(got, d.path().join("skills"));
    }

    #[test]
    fn resolve_within_allows_a_missing_path() {
        let d = tempdir().unwrap();
        let got = resolve_within(d.path(), "./nope").unwrap();
        assert_eq!(got, d.path().join("nope"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_within_rejects_a_symlink_that_escapes_the_root() {
        // Lexically clean, but the link lands outside the plugin root. Only the
        // canonicalizing layer catches this one.
        let outside = tempdir().unwrap();
        fs::create_dir_all(outside.path().join("secrets")).unwrap();
        let d = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path().join("secrets"), d.path().join("skills"))
            .unwrap();
        let e = resolve_within(d.path(), "./skills").unwrap_err();
        assert!(matches!(e, PluginSrcError::PathTraversal(_)), "got {e:?}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_within_allows_a_symlink_that_stays_inside_the_root() {
        // Polarity control for the test above: containment, not symlink-ness,
        // is what is being enforced.
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join("real")).unwrap();
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("skills")).unwrap();
        assert!(resolve_within(d.path(), "./skills").is_ok());
    }
}
