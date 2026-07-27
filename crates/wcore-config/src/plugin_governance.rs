//! F25-04: the plugin approval gate.
//!
//! Approval is a LOAD-TIME GATE, not a confirmation prompt. An installed but
//! unapproved plugin must not execute, and the refusal has to be observable.
//! This module owns the two things the gate is built from:
//!
//! * [`content_digest`] — the SHA-256 identity of a plugin directory, and
//! * [`evaluate`]       — the verdict the loader acts on.
//!
//! It lives in `wcore-config` (rather than in the CLI that writes approvals or
//! the agent that enforces them) because both sides must agree byte-for-byte.
//! Two implementations of "is this plugin approved?" would be two answers to
//! the same security question, which is precisely the defect class F25-04
//! exists to close.
//!
//! ## Enforcement scope — root-scoped governance
//!
//! A plugins root becomes **governed** the moment the lifecycle writes
//! [`GENERATIONS_FILE`] into it (i.e. the first `plugin install` / `approve`
//! through the governed verbs). Inside a governed root EVERY plugin directory
//! needs an approval record whose digest matches its current content, or the
//! loader refuses it. A root carrying no governance state at all behaves
//! exactly as it did before this module existed.
//!
//! That boundary is deliberate and is the one an operator can reason about: it
//! is the *directory* that is governed, not the individual plugin. Scoping the
//! gate per-plugin instead would leave an attacker who simply writes a plugin
//! directory into the root — bypassing the installer — outside the gate
//! entirely, which is the shape of bypass this control exists to stop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Marker + state file that makes a plugins root governed.
pub const GENERATIONS_FILE: &str = "generations.json";

/// Durable approval state, alongside [`GENERATIONS_FILE`] in the plugins root.
pub const APPROVALS_FILE: &str = "approvals.json";

/// Sidecars that live in the plugins ROOT (never inside a plugin directory).
/// Listed here so a future reader does not have to grep for them.
pub const ROOT_SIDECARS: &[&str] = &[
    GENERATIONS_FILE,
    APPROVALS_FILE,
    "installed.lock.json",
    "known_marketplaces.json",
];

/// One recorded approval. Bound to `digest`, so an update — which necessarily
/// changes the digest — invalidates it rather than inheriting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub plugin: String,
    /// SHA-256 of the plugin directory at the moment of approval.
    pub digest: String,
    /// RFC3339, supplied by the caller — this module never reads the clock.
    pub approved_at: String,
}

/// A revocation. Retained (rather than merely deleting the approval) so
/// `plugin recover` can be forbidden from resurrecting an authority a human
/// deliberately withdrew.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationRecord {
    pub plugin: String,
    pub digest: String,
    pub revoked_at: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ApprovalStore {
    #[serde(default)]
    pub approvals: BTreeMap<String, ApprovalRecord>,
    #[serde(default)]
    pub revoked: Vec<RevocationRecord>,
}

/// The loader's verdict for one plugin directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
    /// The root carries no governance state — behave as before this gate existed.
    NotGoverned,
    /// An approval record matches the directory's current digest.
    Approved { digest: String },
    /// Refuse the load. `reason` is operator-facing and reaches stderr.
    Refused { reason: String },
}

/// Prefix on every refusal reason. Callers grep for this exact string, so it is
/// a constant rather than a literal repeated at each site.
pub const REFUSAL_PREFIX: &str = "plugin approval required";

pub fn approvals_path(plugins_root: &Path) -> PathBuf {
    plugins_root.join(APPROVALS_FILE)
}

pub fn generations_path(plugins_root: &Path) -> PathBuf {
    plugins_root.join(GENERATIONS_FILE)
}

/// Is this plugins root under lifecycle governance?
///
/// EITHER marker governs. That disjunction is a binding condition from the
/// cross-audit panel's dissent (see `25-02-CLI-GATE-DECISION.md`): with a single
/// marker, deleting one file silently reverts a governed root to fail-open. With
/// both, un-governing a root requires destroying the approval record itself,
/// which is a loud, self-evident act rather than a quiet one.
pub fn is_governed(plugins_root: &Path) -> bool {
    generations_path(plugins_root).is_file() || approvals_path(plugins_root).is_file()
}

/// Read the approval store. A missing file is an empty store; a CORRUPT file is
/// an error, never an empty store — silently treating unparseable approval
/// state as "no approvals" would turn a damaged file into a fail-open.
pub fn load_approvals(plugins_root: &Path) -> std::io::Result<ApprovalStore> {
    let p = approvals_path(plugins_root);
    if !p.is_file() {
        return Ok(ApprovalStore::default());
    }
    let raw = std::fs::read_to_string(&p)?;
    serde_json::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: {e}", p.display()),
        )
    })
}

/// Persist the approval store atomically.
pub fn store_approvals(plugins_root: &Path, store: &ApprovalStore) -> std::io::Result<()> {
    std::fs::create_dir_all(plugins_root)?;
    let bytes = serde_json::to_vec_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::atomic_write(approvals_path(plugins_root), &bytes)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// SHA-256 identity of a plugin directory.
///
/// Deterministic across platforms: every regular file below `dir` contributes
/// its path (with `\` normalised to `/`) and its bytes, both length-prefixed so
/// no concatenation of one file's tail with the next file's head can collide.
/// Entries are visited in sorted path order. Symlinks are NOT followed — a
/// symlink contributes its own link target as content, so pointing one at a
/// file outside the tree cannot silently change the digest to match.
pub fn content_digest(dir: &Path) -> std::io::Result<String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    hasher.update((files.len() as u64).to_le_bytes());
    for (rel, abs) in &files {
        hasher.update((rel.len() as u64).to_le_bytes());
        hasher.update(rel.as_bytes());
        let meta = std::fs::symlink_metadata(abs)?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(abs)?;
            let t = normalize(&target.to_string_lossy());
            hasher.update(b"L");
            hasher.update((t.len() as u64).to_le_bytes());
            hasher.update(t.as_bytes());
            continue;
        }
        let bytes = std::fs::read(abs)?;
        hasher.update(b"F");
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex(&hasher.finalize()))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map(|p| normalize(&p.to_string_lossy()))
            .unwrap_or_else(|_| normalize(&path.to_string_lossy()));
        out.push((rel, path));
    }
    Ok(())
}

fn normalize(s: &str) -> String {
    s.replace('\\', "/")
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Decide whether `plugin_dir` (named `plugin`) may load.
///
/// The digest is recomputed from the directory on every call: an approval that
/// was granted for different bytes is not an approval for these bytes.
pub fn evaluate(plugins_root: &Path, plugin: &str, plugin_dir: &Path) -> GateVerdict {
    if !is_governed(plugins_root) {
        return GateVerdict::NotGoverned;
    }
    let store = match load_approvals(plugins_root) {
        Ok(s) => s,
        Err(e) => {
            return GateVerdict::Refused {
                reason: format!(
                    "{REFUSAL_PREFIX}: {plugin}: approval store unreadable ({e}) — \
                     refusing rather than treating damaged state as unapproved-but-fine"
                ),
            };
        }
    };
    let digest = match content_digest(plugin_dir) {
        Ok(d) => d,
        Err(e) => {
            return GateVerdict::Refused {
                reason: format!("{REFUSAL_PREFIX}: {plugin}: cannot digest plugin directory ({e})"),
            };
        }
    };
    match store.approvals.get(plugin) {
        Some(rec) if rec.digest == digest => GateVerdict::Approved { digest },
        Some(rec) => GateVerdict::Refused {
            reason: format!(
                "{REFUSAL_PREFIX}: {plugin}: approved digest {} does not match installed digest \
                 {} — the plugin changed since it was approved; re-approve with \
                 `wayland-core plugin approve {plugin}`",
                short(&rec.digest),
                short(&digest)
            ),
        },
        None => GateVerdict::Refused {
            reason: format!(
                "{REFUSAL_PREFIX}: {plugin}: installed at digest {} but never approved — run \
                 `wayland-core plugin approve {plugin}`",
                short(&digest)
            ),
        },
    }
}

/// First 12 hex chars — enough to identify, short enough to read in a log line.
pub fn short(digest: &str) -> String {
    digest.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, body: &[u8]) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    fn plugin_dir(root: &Path, name: &str) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        write(&d, "plugin.toml", b"[plugin]\nname='x'\n");
        d
    }

    #[test]
    fn digest_is_stable_and_order_independent() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        write(a.path(), "z.txt", b"zzz");
        write(a.path(), "sub/a.txt", b"aaa");
        write(b.path(), "sub/a.txt", b"aaa");
        write(b.path(), "z.txt", b"zzz");
        assert_eq!(
            content_digest(a.path()).unwrap(),
            content_digest(b.path()).unwrap()
        );
    }

    #[test]
    fn digest_changes_on_a_single_byte() {
        let a = TempDir::new().unwrap();
        write(a.path(), "f.bin", b"hello");
        let before = content_digest(a.path()).unwrap();
        write(a.path(), "f.bin", b"hellp");
        assert_ne!(before, content_digest(a.path()).unwrap());
    }

    /// Length-prefixing matters: without it, `{"ab": "c"}` and `{"a": "bc"}`
    /// would hash the same byte stream.
    #[test]
    fn digest_is_not_fooled_by_boundary_shifting() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        write(a.path(), "ab", b"c");
        write(b.path(), "a", b"bc");
        assert_ne!(
            content_digest(a.path()).unwrap(),
            content_digest(b.path()).unwrap()
        );
    }

    #[test]
    fn digest_changes_when_a_file_moves_between_paths() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        write(a.path(), "one.txt", b"same");
        write(b.path(), "two.txt", b"same");
        assert_ne!(
            content_digest(a.path()).unwrap(),
            content_digest(b.path()).unwrap()
        );
    }

    #[test]
    fn ungoverned_root_is_not_gated() {
        let root = TempDir::new().unwrap();
        let dir = plugin_dir(root.path(), "demo");
        assert_eq!(
            evaluate(root.path(), "demo", &dir),
            GateVerdict::NotGoverned
        );
    }

    #[test]
    fn governed_root_refuses_an_unapproved_plugin() {
        let root = TempDir::new().unwrap();
        let dir = plugin_dir(root.path(), "demo");
        std::fs::write(generations_path(root.path()), b"{}").unwrap();
        match evaluate(root.path(), "demo", &dir) {
            GateVerdict::Refused { reason } => {
                assert!(reason.contains(REFUSAL_PREFIX), "reason: {reason}");
                assert!(reason.contains("never approved"), "reason: {reason}");
            }
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn approval_admits_then_a_mutation_re_arms_the_refusal() {
        let root = TempDir::new().unwrap();
        let dir = plugin_dir(root.path(), "demo");
        std::fs::write(generations_path(root.path()), b"{}").unwrap();

        let digest = content_digest(&dir).unwrap();
        let mut store = ApprovalStore::default();
        store.approvals.insert(
            "demo".into(),
            ApprovalRecord {
                plugin: "demo".into(),
                digest: digest.clone(),
                approved_at: "2026-07-27T00:00:00Z".into(),
            },
        );
        store_approvals(root.path(), &store).unwrap();
        assert_eq!(
            evaluate(root.path(), "demo", &dir),
            GateVerdict::Approved { digest }
        );

        // Mutate one byte: the approval no longer covers these bytes.
        write(&dir, "plugin.toml", b"[plugin]\nname='y'\n");
        match evaluate(root.path(), "demo", &dir) {
            GateVerdict::Refused { reason } => {
                assert!(reason.contains("does not match"), "reason: {reason}");
            }
            other => panic!("expected refusal after mutation, got {other:?}"),
        }
    }

    /// Binding condition from the panel dissent: deleting ONE marker must not
    /// silently revert a governed root to the ungoverned (fail-open) state.
    #[test]
    fn deleting_one_governance_marker_does_not_un_govern_the_root() {
        let root = TempDir::new().unwrap();
        let dir = plugin_dir(root.path(), "demo");
        std::fs::write(generations_path(root.path()), b"{}").unwrap();
        store_approvals(root.path(), &ApprovalStore::default()).unwrap();
        assert!(is_governed(root.path()));

        std::fs::remove_file(generations_path(root.path())).unwrap();
        assert!(
            is_governed(root.path()),
            "removing generations.json alone must not un-govern the root"
        );
        assert!(matches!(
            evaluate(root.path(), "demo", &dir),
            GateVerdict::Refused { .. }
        ));
    }

    /// A damaged approvals file must refuse, not fail open.
    #[test]
    fn corrupt_approval_store_refuses() {
        let root = TempDir::new().unwrap();
        let dir = plugin_dir(root.path(), "demo");
        std::fs::write(generations_path(root.path()), b"{}").unwrap();
        std::fs::write(approvals_path(root.path()), b"{ this is not json").unwrap();
        match evaluate(root.path(), "demo", &dir) {
            GateVerdict::Refused { reason } => {
                assert!(reason.contains("unreadable"), "reason: {reason}")
            }
            other => panic!("expected refusal on corrupt store, got {other:?}"),
        }
    }
}
