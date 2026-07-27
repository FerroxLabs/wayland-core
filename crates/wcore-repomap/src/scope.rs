//! Git-respecting scope and worktree identity for the persistent index.
//!
//! Two questions live here, and only these two:
//!
//! 1. **What is in scope?** The same `ignore`-crate walk [`crate::RepoMap::build`]
//!    already performs, factored out so the in-memory map and the persistent
//!    store cannot drift apart on what they consider indexable.
//! 2. **Which checkout was the store built against?** A [`ScopeIdentity`] —
//!    the resolved HEAD commit, the symbolic ref it came from, and the git
//!    directory that identifies *this* worktree as distinct from a sibling
//!    linked worktree of the same repository.
//!
//! ## Why the git metadata is read by hand
//!
//! This crate takes no internal `wcore-*` dependency (AGENTS.md), and it also
//! declines a git *library* dependency: `RepoMap` has always been a
//! dependency-light tool, and the three files this module needs — `.git/HEAD`,
//! a loose ref, and `packed-refs` — are a stable, documented, plain-text
//! on-disk format. Parsing them costs ~80 lines and no new package in the
//! workspace lock.
//!
//! ## What a scope change means
//!
//! A [`ScopeIdentity`] mismatch does **not** mean "throw the store away". It
//! means the store's recorded identity is stale and every hit served from it
//! must say so. The store's own refresh path then re-applies incrementally:
//! only records whose *content* differs are touched. Switching branches
//! between two commits that differ in one file costs one re-extraction, not a
//! rebuild.

use std::fs;
use std::path::{Component, Path, PathBuf};

use ignore::WalkBuilder;

use crate::types::{IndexOptions, RepoMapError};

/// Which checkout, and which worktree of it, an index was built against.
///
/// Compared by value: two identities are equal when their head commit,
/// symbolic ref and git directory all match. A repository with no git
/// metadata at all yields an identity whose fields are all `None`, which is
/// still a legitimate — and stable — identity for a plain directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScopeIdentity {
    /// Resolved HEAD commit object id, when it could be read.
    pub head_commit: Option<String>,
    /// Symbolic ref HEAD pointed at (e.g. `refs/heads/main`). `None` for a
    /// detached HEAD, where `head_commit` carries the whole answer.
    pub head_ref: Option<String>,
    /// The resolved git directory. For a linked worktree this is the
    /// per-worktree `…/.git/worktrees/<name>` path, which is precisely what
    /// distinguishes one worktree from another sharing the same object store.
    pub git_dir: Option<String>,
}

impl ScopeIdentity {
    /// Detect the identity of the checkout containing `root`.
    ///
    /// Walks up from `root` looking for a `.git` entry, handling both the
    /// directory form (an ordinary clone) and the file form
    /// (`gitdir: <path>`, a linked worktree or a submodule). Never fails:
    /// unreadable or absent git metadata yields `None` fields rather than an
    /// error, because a repo map over a plain directory is a legitimate use.
    pub fn detect(root: &Path) -> Self {
        let Some(git_dir) = find_git_dir(root) else {
            return Self::default();
        };
        let (head_ref, head_commit) = resolve_head(&git_dir);
        Self {
            head_commit,
            head_ref,
            git_dir: Some(path_to_key(&git_dir)),
        }
    }

    /// A stable single-line encoding, used as the value stored in the index
    /// metadata table and compared on every open.
    ///
    /// Deliberately total: an absent field encodes as `-`, so an identity is
    /// never ambiguous between "no git metadata" and "field missing from an
    /// older store".
    pub fn fingerprint(&self) -> String {
        format!(
            "commit={} ref={} gitdir={}",
            self.head_commit.as_deref().unwrap_or("-"),
            self.head_ref.as_deref().unwrap_or("-"),
            self.git_dir.as_deref().unwrap_or("-"),
        )
    }

    /// Parse a [`Self::fingerprint`] back into an identity.
    ///
    /// Returns `None` when the string was not produced by `fingerprint` —
    /// which is how a store written by an incompatible schema is detected
    /// rather than silently mis-compared.
    pub fn from_fingerprint(s: &str) -> Option<Self> {
        let mut commit = None;
        let mut head_ref = None;
        let mut git_dir = None;
        let mut seen = 0;
        for part in s.splitn(3, ' ') {
            let (k, v) = part.split_once('=')?;
            let v = if v == "-" { None } else { Some(v.to_string()) };
            match k {
                "commit" => {
                    commit = v;
                    seen += 1;
                }
                "ref" => {
                    head_ref = v;
                    seen += 1;
                }
                "gitdir" => {
                    git_dir = v;
                    seen += 1;
                }
                _ => return None,
            }
        }
        if seen != 3 {
            return None;
        }
        Some(Self {
            head_commit: commit,
            head_ref,
            git_dir,
        })
    }
}

/// One in-scope file, as the scope walk sees it before anything is read.
///
/// `size_bytes` and `mtime_unix_nanos` are taken from the directory entry's
/// metadata, which the walk already had to `stat`. They are what lets the
/// store decide a file is unchanged **without opening it** — the property the
/// incremental suite proves by counting reads rather than by timing.
#[derive(Debug, Clone)]
pub struct ScopeEntry {
    /// Canonical relative path key. Always `/`-separated, produced by
    /// [`normalize_rel`], and identical on every platform.
    pub key: String,
    /// Absolute path on this host.
    pub abs_path: PathBuf,
    /// Size in bytes, from the walk's own `stat`.
    pub size_bytes: u64,
    /// Modification time in nanoseconds since the Unix epoch, or `0` when the
    /// platform or filesystem does not report one.
    pub mtime_unix_nanos: u128,
}

/// Canonical relative-path key.
///
/// **This is the Windows correctness boundary.** `std::fs::canonicalize`
/// yields a verbatim `\\?\` extended-length prefix on Windows, and the
/// simplification helpers that undo it conditionally no-op on components over
/// 255 characters, non-UTF-8 names, reserved DOS names, and trailing dots or
/// spaces. An index that *stores* one representation and *looks up* another
/// passes every Linux test and silently misses on Windows.
///
/// The rule this crate follows is: normalise at the **comparison boundary, on
/// both operands**. Every path written to the store and every path used to
/// query it goes through this function, which keeps only
/// [`Component::Normal`] parts and joins them with `/`. Prefix, root-dir and
/// parent components cannot appear in a `strip_prefix` result and are dropped
/// if a caller passes one anyway, so a lookup key can never escape the root
/// through this function.
pub fn normalize_rel(rel: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for component in rel.components() {
        if let Component::Normal(part) = component {
            parts.push(part.to_string_lossy().into_owned());
        }
    }
    parts.join("/")
}

/// Enumerate every in-scope file under `root`.
///
/// Uses the walk [`crate::RepoMap::build_with_options`] uses — standard
/// filters bound to `respect_gitignore`, hidden entries included so dotfiles
/// are visible while gitignore still applies, and per-entry errors skipped in
/// the crate's light-tool stance. Symlinks are **not** followed:
/// `WalkBuilder` defaults to `follow_links(false)`, so a symlink pointing
/// outside the indexed root is never opened, never hashed and never stored.
///
/// ## The one deliberate divergence: `.git` is excluded
///
/// The in-memory walk sets `.hidden(false)`, which re-admits dotfiles *and*
/// the `.git` directory itself. For a one-shot map that is merely noisy. For
/// a **persistent** store it is wrong three ways: the object store is not
/// source and pollutes retrieval; `.git/logs`, `COMMIT_EDITMSG` and the index
/// churn on every git operation, so a store that indexed them could never
/// report an honest "nothing changed"; and persisting repository internals
/// widens what a backed-up store file contains. `.git` is therefore dropped
/// here by name. `RepoMap::build` is untouched — this is the persistent
/// index's scope, not a change to the existing API's behaviour.
///
/// # Errors
///
/// Returns [`RepoMapError::Root`] when `root` cannot be canonicalised.
pub fn scope_files(root: &Path, opts: &IndexOptions) -> Result<Vec<ScopeEntry>, RepoMapError> {
    let canonical = fs::canonicalize(root).map_err(|e| RepoMapError::Root {
        path: root.to_path_buf(),
        source: e,
    })?;

    let mut entries = Vec::new();
    let walker = WalkBuilder::new(&canonical)
        .standard_filters(opts.respect_gitignore)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != std::ffi::OsStr::new(".git"))
        .build();

    for entry in walker {
        let Ok(entry) = entry else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        // `is_file()` is false for a symlink (links are not followed), so a
        // link out of the tree is dropped here, before any read.
        if !file_type.is_file() {
            continue;
        }
        let abs_path = entry.path();
        // Defence in depth: a path that does not sit under the root is not
        // storable, regardless of how the walker produced it.
        let Ok(rel) = abs_path.strip_prefix(&canonical) else {
            continue;
        };
        let Ok(metadata) = fs::metadata(abs_path) else {
            continue;
        };
        entries.push(ScopeEntry {
            key: normalize_rel(rel),
            abs_path: abs_path.to_path_buf(),
            size_bytes: metadata.len(),
            mtime_unix_nanos: mtime_nanos(&metadata),
        });
    }

    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(entries)
}

fn mtime_nanos(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn path_to_key(path: &Path) -> String {
    // Slash-normalised so a fingerprint compares equal across the shells and
    // path styles a single Windows host can present.
    path.to_string_lossy().replace('\\', "/")
}

/// Walk up from `start` looking for git metadata, returning the resolved git
/// directory.
fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let mut cursor: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cursor {
        let candidate = dir.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            // Linked worktree or submodule: `gitdir: <path>`.
            let text = fs::read_to_string(&candidate).ok()?;
            let target = text.trim().strip_prefix("gitdir:")?.trim();
            let target_path = Path::new(target);
            let resolved = if target_path.is_absolute() {
                target_path.to_path_buf()
            } else {
                dir.join(target_path)
            };
            return fs::canonicalize(&resolved).ok().or(Some(resolved));
        }
        cursor = dir.parent();
    }
    None
}

/// Resolve `HEAD` inside `git_dir` to `(symbolic ref, commit)`.
fn resolve_head(git_dir: &Path) -> (Option<String>, Option<String>) {
    let Ok(head) = fs::read_to_string(git_dir.join("HEAD")) else {
        return (None, None);
    };
    let head = head.trim();
    let Some(symbolic) = head.strip_prefix("ref:") else {
        // Detached HEAD: the file holds the object id itself.
        return (None, Some(head.to_string()));
    };
    let symbolic = symbolic.trim().to_string();

    // A linked worktree keeps its own HEAD but shares refs with the common
    // directory, so both are consulted.
    let mut roots = vec![git_dir.to_path_buf()];
    if let Ok(common) = fs::read_to_string(git_dir.join("commondir")) {
        let common = common.trim();
        let common_path = Path::new(common);
        roots.push(if common_path.is_absolute() {
            common_path.to_path_buf()
        } else {
            git_dir.join(common_path)
        });
    }

    for root in &roots {
        if let Ok(oid) = fs::read_to_string(root.join(&symbolic)) {
            let oid = oid.trim();
            if !oid.is_empty() {
                return (Some(symbolic), Some(oid.to_string()));
            }
        }
    }
    for root in &roots {
        if let Ok(packed) = fs::read_to_string(root.join("packed-refs")) {
            for line in packed.lines() {
                let line = line.trim();
                if line.starts_with('#') || line.starts_with('^') {
                    continue;
                }
                if let Some((oid, name)) = line.split_once(' ') {
                    if name.trim() == symbolic {
                        return (Some(symbolic), Some(oid.trim().to_string()));
                    }
                }
            }
        }
    }
    // The ref exists but is unborn (a fresh `git init`). That is a real,
    // stable identity: "on this branch, no commit yet".
    (Some(symbolic), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rel_is_slash_separated_and_drops_non_normal_components() {
        assert_eq!(normalize_rel(Path::new("src/lib.rs")), "src/lib.rs");
        assert_eq!(normalize_rel(Path::new("./src/lib.rs")), "src/lib.rs");
        // A caller that hands in an absolute path cannot smuggle the root
        // into the key; only Normal components survive.
        let abs = normalize_rel(Path::new("/etc/passwd"));
        assert_eq!(abs, "etc/passwd");
        assert!(!abs.starts_with('/'));
    }

    #[test]
    fn normalize_rel_round_trips_a_non_ascii_component() {
        // The Windows path-simplification helper conditionally no-ops on
        // non-UTF-8 and unusual names; a key that survives round-tripping
        // through `Path` is what makes a lookup hit on that platform.
        let key = normalize_rel(Path::new("src/ünïcode/módulo.rs"));
        assert_eq!(key, "src/ünïcode/módulo.rs");
        assert_eq!(normalize_rel(Path::new(&key)), key);
    }

    #[test]
    fn normalize_rel_round_trips_a_deeply_nested_path() {
        let deep: PathBuf = (0..24).map(|i| format!("d{i}")).collect();
        let deep = deep.join("leaf.rs");
        let key = normalize_rel(&deep);
        assert_eq!(key.matches('/').count(), 24);
        assert_eq!(normalize_rel(Path::new(&key)), key);
    }

    #[test]
    fn fingerprint_round_trips_including_absent_fields() {
        let id = ScopeIdentity {
            head_commit: Some("abc123".into()),
            head_ref: Some("refs/heads/main".into()),
            git_dir: Some("/repo/.git".into()),
        };
        assert_eq!(ScopeIdentity::from_fingerprint(&id.fingerprint()), Some(id));

        let empty = ScopeIdentity::default();
        assert_eq!(
            ScopeIdentity::from_fingerprint(&empty.fingerprint()),
            Some(empty)
        );
    }

    #[test]
    fn from_fingerprint_rejects_a_foreign_string() {
        assert_eq!(ScopeIdentity::from_fingerprint("not a fingerprint"), None);
        assert_eq!(ScopeIdentity::from_fingerprint("commit=a ref=b"), None);
    }

    #[test]
    fn detect_on_a_plain_directory_is_a_stable_empty_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = ScopeIdentity::detect(dir.path());
        let b = ScopeIdentity::detect(dir.path());
        assert_eq!(a, b);
    }
}
