//! Stable content digest over a directory tree (F26-01).
//!
//! # Why this exists
//!
//! "Discovery does not mutate the tree it previews" is a claim about behavior,
//! and the only honest way to hold it is to measure it: digest the source
//! before a dry-run, digest it after, and require the two to be identical. A
//! reader of the discovery code can convince themselves it only opens files for
//! reading; a digest catches the case where they were wrong.
//!
//! # Symlink discipline
//!
//! A peer state tree is attacker-influenced by construction. Following a
//! symlink out of the declared root would let a hostile state directory make
//! the digest — and, worse, the discovery walk that shares this discipline —
//! read arbitrary files elsewhere on the machine. So a symlink is never
//! traversed: it is recorded as a link, and if it resolves outside the root it
//! is additionally reported as an escape, surfaced to the caller as a warning
//! rather than silently skipped. This mirrors `profile::copy_tree_filtered`'s
//! existing rule (C6: no symlink/junction sharing).

use std::collections::BTreeMap;
use std::path::{Component, Path};

use sha2::{Digest, Sha256};

/// The digest of a directory tree, plus what the walk refused to follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDigest {
    /// Hex SHA-256 over the whole tree — the value compared before and after.
    pub digest: String,
    /// Number of regular files that contributed content.
    pub files: usize,
    /// Symlinks whose target resolves OUTSIDE the digested root, as
    /// root-relative paths. Non-empty means the tree tried to reach out of
    /// itself; the caller surfaces these as plan warnings.
    pub symlink_escapes: Vec<String>,
}

/// Digest every regular file under `root`, deterministically.
///
/// Ordering is total and derived from the data (root-relative path), never from
/// directory iteration order, so two walks of the same tree cannot differ. Each
/// entry contributes its relative path AND its content hash, so a pure rename
/// changes the digest just as a content edit does.
///
/// Unreadable entries do not abort the walk — a real peer tree contains
/// directories the current user cannot open — but they DO contribute a distinct
/// marker, so an entry becoming unreadable is a change rather than a silent
/// no-op.
pub fn tree_digest(root: &Path) -> std::io::Result<TreeDigest> {
    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    let mut escapes: Vec<String> = Vec::new();
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    walk(root, root, &canonical_root, &mut entries, &mut escapes)?;

    let files = entries.len();
    let mut hasher = Sha256::new();
    for (rel, content_hash) in &entries {
        // Length-prefix each field so that no combination of path and hash can
        // be re-parsed as a different pair.
        hasher.update(rel.len().to_le_bytes());
        hasher.update(rel.as_bytes());
        hasher.update(content_hash.as_bytes());
    }
    escapes.sort();
    escapes.dedup();

    Ok(TreeDigest {
        digest: hex(&hasher.finalize()),
        files,
        symlink_escapes: escapes,
    })
}

fn walk(
    root: &Path,
    dir: &Path,
    canonical_root: &Path,
    entries: &mut BTreeMap<String, String>,
    escapes: &mut Vec<String>,
) -> std::io::Result<()> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => {
            // Unreadable directory: record it as such and keep going.
            if let Some(rel) = relative(root, dir) {
                entries.insert(rel, "<unreadable-dir>".to_string());
            }
            return Ok(());
        }
    };

    // Collect first so the recursion order is sorted and therefore stable.
    let mut children: Vec<std::path::PathBuf> =
        rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    children.sort();

    for path in children {
        // symlink_metadata: detect the LINK, never the thing it points at.
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let Some(rel) = relative(root, &path) else {
            continue;
        };

        if meta.file_type().is_symlink() {
            // Never traversed. Recorded by its target text so that repointing a
            // link changes the digest.
            let target = std::fs::read_link(&path)
                .map(|t| t.to_string_lossy().into_owned())
                .unwrap_or_default();
            entries.insert(rel.clone(), format!("<symlink:{target}>"));
            if escapes_root(&path, canonical_root) {
                escapes.push(rel);
            }
            continue;
        }

        if meta.is_dir() {
            walk(root, &path, canonical_root, entries, escapes)?;
        } else if meta.is_file() {
            let hash = match std::fs::read(&path) {
                Ok(bytes) => {
                    let mut h = Sha256::new();
                    h.update(&bytes);
                    hex(&h.finalize())
                }
                Err(_) => "<unreadable-file>".to_string(),
            };
            entries.insert(rel, hash);
        }
    }
    Ok(())
}

/// True when a symlink's resolved target lies outside `canonical_root`.
///
/// A link that dangles cannot be proven to stay inside, so it is treated as an
/// escape: fail toward reporting rather than toward silence.
fn escapes_root(link: &Path, canonical_root: &Path) -> bool {
    match link.canonicalize() {
        Ok(resolved) => !resolved.starts_with(canonical_root),
        Err(_) => true,
    }
}

/// Root-relative, `/`-separated path. Returns `None` for anything that is not
/// genuinely under `root`.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    // Reject any `..` that survived, so a traversal component can never be
    // recorded as though it were a normal name.
    if rel.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    Some(
        rel.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_across_two_walks_and_sensitive_to_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/one.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("a/b/two.txt"), "world").unwrap();

        let d1 = tree_digest(dir.path()).unwrap();
        let d2 = tree_digest(dir.path()).unwrap();
        assert_eq!(d1, d2, "two walks of one tree must agree");
        assert_eq!(d1.files, 2);

        // Negative control: the digest must actually MOVE when content changes,
        // otherwise the non-mutation proof would pass no matter what.
        std::fs::write(dir.path().join("a/one.txt"), "hello!").unwrap();
        let d3 = tree_digest(dir.path()).unwrap();
        assert_ne!(d1.digest, d3.digest, "digest ignored a content change");
    }

    #[test]
    fn digest_changes_when_a_file_is_renamed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("one.txt"), "same").unwrap();
        let before = tree_digest(dir.path()).unwrap().digest;
        std::fs::rename(dir.path().join("one.txt"), dir.path().join("two.txt")).unwrap();
        let after = tree_digest(dir.path()).unwrap().digest;
        assert_ne!(
            before, after,
            "a rename with identical content must change the digest"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_root_is_reported_and_never_followed() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "OUTSIDE-CONTENT").unwrap();

        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("inside.txt"), "inside").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join("escape"),
        )
        .unwrap();

        let d = tree_digest(root.path()).unwrap();
        assert_eq!(
            d.symlink_escapes,
            vec!["escape".to_string()],
            "an escaping symlink must be reported, not silently skipped"
        );
        // Only the real file contributed content; the link contributed its
        // target text, and the tree it pointed at was never read.
        assert_eq!(d.files, 2, "link is recorded as an entry, not traversed");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_the_root_is_not_an_escape() {
        // Negative control for the test above: without it, an implementation
        // that flagged EVERY symlink would look correct.
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("real.txt"), "x").unwrap();
        std::os::unix::fs::symlink(root.path().join("real.txt"), root.path().join("link")).unwrap();

        let d = tree_digest(root.path()).unwrap();
        assert!(
            d.symlink_escapes.is_empty(),
            "a link that stays inside the root is not an escape: {:?}",
            d.symlink_escapes
        );
    }
}
