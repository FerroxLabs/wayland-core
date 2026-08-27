//! Resolution for `@`-references — turning a parsed [`AtRef`] into the
//! [`AtPayload`] a message carries.
//!
//! This module owns the [`AtPayload`] / [`ResolvedFile`] / [`AtWarning`]
//! types and the [`resolve`] entry point. Filesystem kinds (`@file`,
//! `@dir`) are read here under the secret + gitignore guardrails from
//! [`at_ref_guard`]; the network/engine kinds (`@url`, `@session`,
//! `@symbol`, `@diff`) resolve to deferred placeholders whose real work
//! happens behind the protocol bridge. Split out of `at_refs.rs` (W3-B).

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::at_ref_guard::{GitIgnore, GuardedFile, is_secret_path, is_secret_target};
use super::at_ref_parse::{AtRef, AtRefError};

// ─────────────────────────────────────────────────────────────────────────
// Tunables
// ─────────────────────────────────────────────────────────────────────────

/// Characters-per-token divisor for the cost estimate. The engine's real
/// tokenizer lives behind the provider boundary and is not reachable from
/// the TUI crate; `~4 chars/token` is the standard heuristic for English +
/// code and is good enough for a *budget preview* (it never gates a send,
/// it only sizes a chip and triggers a warning).
const CHARS_PER_TOKEN: usize = 4;

/// Token budget above which an `@dir` resolution warns. Roughly an eighth
/// of a 200k-token window — a directory that large almost always wants the
/// names-only fallback rather than every file's full contents inlined.
pub const DIR_TOKEN_WARN_BUDGET: usize = 25_000;

/// Hard cap on files pulled by a single `@dir` resolution. A pathological
/// tree (`node_modules`, `target/`) must not be walked without bound even
/// when it is not git-ignored.
const DIR_MAX_FILES: usize = 2_000;

// ─────────────────────────────────────────────────────────────────────────
// AtPayload — the resolved content a message carries
// ─────────────────────────────────────────────────────────────────────────

/// One resolved file inside an [`AtPayload`]: its path and contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFile {
    /// The file's path, relative to the resolution root where possible.
    pub path: PathBuf,
    /// The file's contents. For a names-only `@dir` this is empty and only
    /// `path` is meaningful.
    pub content: String,
}

/// A non-fatal advisory raised during resolution. Resolution still
/// succeeds — the composer decides whether to act on the warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtWarning {
    /// An `@dir` tree exceeded [`DIR_TOKEN_WARN_BUDGET`]. Carries the
    /// estimated token cost so the composer can offer a names-only attach.
    OversizedDir {
        /// The estimated token cost of the full-contents tree.
        tokens: usize,
    },
    /// One or more files in an `@dir` walk were skipped because they are
    /// git-ignored or secret. Carries the count for an honest "N skipped".
    SkippedFiles {
        /// How many files the walk skipped.
        count: usize,
    },
    /// The `@dir` walk hit [`DIR_MAX_FILES`] and stopped early.
    Truncated {
        /// The cap that was hit.
        limit: usize,
    },
}

impl fmt::Display for AtWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AtWarning::OversizedDir { tokens } => {
                write!(
                    f,
                    "directory tree is large (~{tokens} tokens) — consider names-only"
                )
            }
            AtWarning::SkippedFiles { count } => {
                write!(f, "{count} file(s) skipped (git-ignored or secret)")
            }
            AtWarning::Truncated { limit } => {
                write!(f, "directory tree truncated at {limit} files")
            }
        }
    }
}

/// The resolved payload an `@`-reference contributes to the next message.
///
/// The composer turns this into the `Message.files` / content payload at
/// send time (Wave 2). It is provider-neutral on purpose — just paths,
/// text, and a size estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtPayload {
    /// The reference this payload resolved from.
    pub kind: PayloadKind,
    /// Files carried by the payload. Empty for purely textual payloads
    /// (`@diff`, `@url`, `@output`) and for an unresolved `@symbol`.
    pub files: Vec<ResolvedFile>,
    /// Free-text content carried by the payload (a diff, a fetched page, a
    /// symbol body). Empty when the payload is purely file-based.
    pub text: String,
    /// Advisories raised during resolution. Empty on a clean resolve.
    pub warnings: Vec<AtWarning>,
}

/// The flavor of a resolved [`AtPayload`], for the composer's chip label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    /// Resolved from `@file`.
    File,
    /// Resolved from `@dir` (full contents).
    Dir,
    /// Resolved from `@dir` (names only — the oversized fallback).
    DirNamesOnly,
    /// Resolved from `@symbol`.
    Symbol,
    /// Resolved from `@diff`.
    Diff,
    /// Resolved from `@url` (deferred — the actual fetch is Wave 2).
    Url,
    /// Resolved from `@session` (deferred — the lookup is Wave 2).
    Session,
    /// Resolved from `@output`.
    Output,
}

impl AtPayload {
    /// Total byte size of the payload: every file's content plus the free
    /// text.
    pub fn bytes(&self) -> usize {
        self.text.len() + self.files.iter().map(|f| f.content.len()).sum::<usize>()
    }

    /// Estimated token cost — the number shown on the composer chip
    /// (`@compat.rs ≈ 7k tokens`). A `~4 chars/token` heuristic; see
    /// [`CHARS_PER_TOKEN`] for why an estimate is acceptable here.
    pub fn tokens(&self) -> usize {
        self.bytes().div_ceil(CHARS_PER_TOKEN)
    }

    /// True if any advisory was raised.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// Estimate the token cost of an arbitrary text blob, using the same
/// heuristic [`AtPayload::tokens`] applies. Exposed so the completion
/// popup can preview a candidate's cost before it is resolved.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(CHARS_PER_TOKEN)
}

// ─────────────────────────────────────────────────────────────────────────
// Resolution
// ─────────────────────────────────────────────────────────────────────────

/// Resolve a parsed [`AtRef`] into the [`AtPayload`] a message will carry.
///
/// `root` is the workspace directory that file/dir references resolve
/// relative to and that the `.gitignore` is loaded from.
///
/// The network-backed and engine-backed kinds (`@url`, `@session`) do NOT
/// fetch here — that work belongs to Wave 2, behind the protocol bridge.
/// They resolve to a *deferred* placeholder payload so the composer can
/// still show a chip and surface the network seam in the UI.
pub fn resolve(at: &AtRef, root: &Path) -> Result<AtPayload, AtRefError> {
    match at {
        AtRef::File(path) => resolve_file(path, root),
        AtRef::Dir(path) => resolve_dir(path, root),
        AtRef::Symbol(name) => Ok(resolve_symbol(name)),
        AtRef::Diff { base } => Ok(resolve_diff(base.as_deref())),
        AtRef::Url(url) => Ok(resolve_deferred(PayloadKind::Url, url)),
        AtRef::Session(id) => Ok(resolve_deferred(PayloadKind::Session, id)),
        AtRef::Output => Ok(resolve_deferred(PayloadKind::Output, "")),
    }
}

/// Resolve `@file`: read one file, honoring the secret + gitignore guards.
///
/// The file is opened exactly once and every guard judges the object that
/// open resolved to, not the spelling the user typed — see [`GuardedFile`].
fn resolve_file(path: &Path, root: &Path) -> Result<AtPayload, AtRefError> {
    let full = resolve_under_root(path, root);

    // Gate 1 — the caller's own spelling, before the filesystem is touched,
    // so `@.env` is refused whether or not the file exists.
    if is_secret_path(&full) {
        return Err(AtRefError::SecretBlocked(display(path)));
    }

    // One open. Every gate below judges this handle's proven identity, and
    // the read below consumes the same handle.
    let mut opened = GuardedFile::open(&full).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput => {
            AtRefError::NotFound(display(path))
        }
        _ => AtRefError::Io {
            path: display(path),
            message: e.to_string(),
        },
    })?;

    // Gate 2 — the resolved target's own name (core#339).
    if is_secret_target(&full, opened.resolved()) {
        return Err(AtRefError::SecretBlocked(display(path)));
    }
    // Gate 3 — gitignore, judged on the resolved path against the resolved
    // root, so a `..` component or a root spelled through a symlink can no
    // longer land the path outside the matcher's reach (core#335).
    if let Some(rel) = rel_to_canonical_root(opened.resolved(), root)
        && GitIgnore::load(root).is_ignored(&rel, false)
    {
        return Err(AtRefError::GitIgnored(display(path)));
    }

    let content = opened.read_to_string().map_err(|e| AtRefError::Io {
        path: display(path),
        message: e.to_string(),
    })?;

    Ok(AtPayload {
        kind: PayloadKind::File,
        files: vec![ResolvedFile {
            path: path.to_path_buf(),
            content,
        }],
        text: String::new(),
        warnings: Vec::new(),
    })
}

/// Resolve `@dir`: walk a directory tree, reading file contents, skipping
/// git-ignored and secret files. An oversized tree resolves with an
/// [`AtWarning::OversizedDir`] so the composer can offer names-only.
fn resolve_dir(path: &Path, root: &Path) -> Result<AtPayload, AtRefError> {
    let full = resolve_under_root(path, root);
    if !full.is_dir() {
        return Err(AtRefError::NotFound(display(path)));
    }
    // The walk runs entirely in resolved coordinates: the root the payload
    // paths are relative to, and the tree the walk descends, are both the
    // symlink-free spellings. That is what makes `rel_to_root` a real
    // containment test rather than a string comparison (core#335).
    let canon_root = fs::canonicalize(root).map_err(|e| AtRefError::Io {
        path: display(root),
        message: e.to_string(),
    })?;
    let start = fs::canonicalize(&full).map_err(|e| AtRefError::Io {
        path: display(path),
        message: e.to_string(),
    })?;

    let ignore = GitIgnore::load(root);
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped = 0usize;
    let mut truncated = false;
    let mut visited: HashSet<PathBuf> = HashSet::new();
    visited.insert(start.clone());

    walk_dir(
        &start,
        &canon_root,
        &ignore,
        &mut files,
        &mut skipped,
        &mut truncated,
        &mut visited,
    )?;

    if truncated {
        warnings.push(AtWarning::Truncated {
            limit: DIR_MAX_FILES,
        });
    }
    if skipped > 0 {
        warnings.push(AtWarning::SkippedFiles { count: skipped });
    }

    let total_bytes: usize = files.iter().map(|f| f.content.len()).sum();
    let tokens = total_bytes.div_ceil(CHARS_PER_TOKEN);
    let (kind, files) = if tokens > DIR_TOKEN_WARN_BUDGET {
        warnings.push(AtWarning::OversizedDir { tokens });
        // Over budget: degrade to names-only — drop the file bodies so the
        // payload the composer holds is the safe fallback by default.
        let names: Vec<ResolvedFile> = files
            .into_iter()
            .map(|f| ResolvedFile {
                path: f.path,
                content: String::new(),
            })
            .collect();
        (PayloadKind::DirNamesOnly, names)
    } else {
        (PayloadKind::Dir, files)
    };

    Ok(AtPayload {
        kind,
        files,
        text: String::new(),
        warnings,
    })
}

/// Depth-first directory walk for `@dir`, applying both guardrails.
///
/// `dir` and `root` are both already canonical. `visited` holds the
/// canonical directories already descended, which is what makes a
/// self-referential symlink terminate — a name-keyed check cannot see that
/// `a/link` and `a` are the same directory.
#[allow(clippy::too_many_arguments)]
fn walk_dir(
    dir: &Path,
    root: &Path,
    ignore: &GitIgnore,
    out: &mut Vec<ResolvedFile>,
    skipped: &mut usize,
    truncated: &mut bool,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), AtRefError> {
    if *truncated {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| AtRefError::Io {
        path: display(dir),
        message: e.to_string(),
    })?;

    // Sort entries for a deterministic walk — the payload (and its tests)
    // must not depend on filesystem iteration order.
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();

    for path in paths {
        if out.len() >= DIR_MAX_FILES {
            *truncated = true;
            return Ok(());
        }
        // `path` is the entry as this workspace spells it — the spelling
        // the user's `.gitignore` patterns are written against.
        let rel = match rel_to_root(&path, root) {
            Some(r) => r,
            None => continue,
        };
        // `metadata` follows the link, so this is the kind of the object,
        // not the kind of the name. An unreadable or broken entry is
        // skipped honestly rather than guessed at.
        let is_dir = match fs::metadata(&path) {
            Ok(m) => m.is_dir(),
            Err(_) => {
                *skipped += 1;
                continue;
            }
        };
        if ignore.is_ignored(&rel, is_dir) {
            *skipped += 1;
            continue;
        }
        if is_dir {
            let Ok(resolved) = fs::canonicalize(&path) else {
                *skipped += 1;
                continue;
            };
            // `.git` is always skipped — it is never useful context and
            // can be enormous. Checked on both spellings so a link cannot
            // rename it in.
            if path.file_name().and_then(|n| n.to_str()) == Some(".git")
                || resolved.file_name().and_then(|n| n.to_str()) == Some(".git")
            {
                continue;
            }
            // A directory link whose target leaves the workspace would pull
            // an arbitrary out-of-tree slice into an `@dir` of *this*
            // workspace, under this workspace's gitignore — which does not
            // govern it. Skipping is counted, not silent, and removes no
            // way to attach anything: `@<path>` still resolves a path
            // outside the root when the user names it deliberately.
            let Some(resolved_rel) = rel_to_root(&resolved, root) else {
                *skipped += 1;
                continue;
            };
            if ignore.is_ignored(&resolved_rel, true) {
                *skipped += 1;
                continue;
            }
            // Identity, not name, is what makes a cycle detectable.
            if !visited.insert(resolved.clone()) {
                continue;
            }
            walk_dir(&resolved, root, ignore, out, skipped, truncated, visited)?;
        } else {
            // One open; both remaining gates judge its proven identity.
            // Read text files only — a binary file is skipped silently
            // rather than corrupting the payload with lossy bytes.
            let mut opened = match GuardedFile::open(&path) {
                Ok(f) => f,
                Err(_) => {
                    *skipped += 1;
                    continue;
                }
            };
            if is_secret_target(&path, opened.resolved()) {
                *skipped += 1;
                continue;
            }
            if rel_to_root(opened.resolved(), root)
                .is_some_and(|r| ignore.is_ignored(&r, false))
            {
                *skipped += 1;
                continue;
            }
            match opened.read_to_string() {
                Ok(content) => out.push(ResolvedFile {
                    path: PathBuf::from(&rel),
                    content,
                }),
                Err(_) => *skipped += 1,
            }
        }
    }
    Ok(())
}

/// Resolve `@symbol`. The repomap symbol index lives behind a Wave-2
/// wiring point, so this produces a deferred placeholder payload: the
/// composer shows a chip, the real definition + call-site lookup is
/// filled when the index is bound in.
fn resolve_symbol(name: &str) -> AtPayload {
    AtPayload {
        kind: PayloadKind::Symbol,
        files: Vec::new(),
        text: format!("@symbol {name} (resolved from the repomap index at send time)"),
        warnings: Vec::new(),
    }
}

/// Resolve `@diff`. The working-tree (or `@diff <ref>`) diff is produced
/// by the engine's git tooling at send time; this records the request as a
/// textual placeholder the composer turns into a chip.
fn resolve_diff(base: Option<&str>) -> AtPayload {
    let text = match base {
        Some(r) => format!("@diff vs {r} (working-tree diff, resolved at send time)"),
        None => "@diff (working-tree diff, resolved at send time)".to_string(),
    };
    AtPayload {
        kind: PayloadKind::Diff,
        files: Vec::new(),
        text,
        warnings: Vec::new(),
    }
}

/// Build a deferred placeholder payload for a kind whose real resolution
/// (a network fetch, a session lookup, the last shell output) happens in
/// Wave 2 behind the protocol bridge.
fn resolve_deferred(kind: PayloadKind, target: &str) -> AtPayload {
    let text = match kind {
        PayloadKind::Url => format!("@url {target} (fetched + readability-extracted at send time)"),
        PayloadKind::Session => {
            format!("@session {target} (loaded as reference context at send time)")
        }
        PayloadKind::Output => {
            "@output (last shell command stdout/stderr, captured at send time)".to_string()
        }
        _ => target.to_string(),
    };
    AtPayload {
        kind,
        files: Vec::new(),
        text,
        warnings: Vec::new(),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Path helpers
// ─────────────────────────────────────────────────────────────────────────

/// Join `path` under `root` if it is relative; an absolute `path` is taken
/// as-is.
fn resolve_under_root(path: &Path, root: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// The path of `full` relative to `root`, as a `/`-joined string, if
/// `full` is inside `root`. Returns `None` for a path that escapes the
/// root (a `..` traversal or an unrelated absolute path) — such a path is
/// outside the gitignore's jurisdiction and is treated conservatively by
/// the caller.
fn rel_to_root(full: &Path, root: &Path) -> Option<String> {
    let stripped = full.strip_prefix(root).ok()?;
    // Reject any residual `..` — a relative path that climbs out of root.
    if stripped
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return None;
    }
    let joined: Vec<String> = stripped
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(str::to_string),
            _ => None,
        })
        .collect();
    if joined.is_empty() {
        None
    } else {
        Some(joined.join("/"))
    }
}

/// The path of an already-resolved `resolved` relative to `root`, with
/// `root` canonicalized first.
///
/// `rel_to_root` is a lexical prefix test, so it answers "is this inside the
/// workspace?" only when both sides are spelled the same way. They routinely
/// are not: a `..` component, or a root reached through a symlink (`/tmp` is
/// `/private/tmp` on macOS), makes a path that really is inside the
/// workspace fail the prefix test — and a `None` here means the caller skips
/// the gitignore guard entirely (core#335). Canonicalizing both sides makes
/// the test about containment rather than spelling.
///
/// Still `None` for a target genuinely outside the workspace: this root's
/// `.gitignore` has no jurisdiction over `/etc/hosts`, and pretending
/// otherwise would be the wrong guard, not a stronger one. Such a path is
/// still subject to both name gates.
fn rel_to_canonical_root(resolved: &Path, root: &Path) -> Option<String> {
    let canonical_root = fs::canonicalize(root).ok()?;
    rel_to_root(resolved, &canonical_root)
}

/// A lossy display string for a path, for error messages.
fn display(path: &Path) -> String {
    path.display().to_string()
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── secret + gitignore guard, end to end ─────────────────────────────

    #[test]
    fn resolving_a_dotenv_file_is_a_loud_error() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".env"), "SECRET_KEY=hunter2").expect("write .env");

        let at = AtRef::parse("@.env").expect("parse");
        let err = resolve(&at, root).expect_err("must refuse a secret");
        assert!(matches!(err, AtRefError::SecretBlocked(_)));
    }

    #[test]
    fn an_at_dir_walk_never_pulls_in_a_secret_file() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("ok.txt"), "safe").expect("write ok");
        fs::write(root.join(".env"), "SECRET=1").expect("write .env");
        fs::write(root.join("server.pem"), "-----BEGIN KEY-----").expect("write pem");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");
        let names: Vec<_> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(names.iter().any(|n| n.contains("ok.txt")));
        assert!(!names.iter().any(|n| n.contains(".env")));
        assert!(!names.iter().any(|n| n.contains("server.pem")));
        // Two secrets were skipped — surfaced honestly.
        assert!(
            payload
                .warnings
                .iter()
                .any(|w| matches!(w, AtWarning::SkippedFiles { count: 2 }))
        );
    }

    #[test]
    fn resolving_a_gitignored_file_is_refused() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "secret.txt\n").expect("write gitignore");
        fs::write(root.join("secret.txt"), "ignored body").expect("write file");

        let at = AtRef::parse("@secret.txt").expect("parse");
        let err = resolve(&at, root).expect_err("git-ignored file refused");
        assert!(matches!(err, AtRefError::GitIgnored(_)));
    }

    #[test]
    fn an_at_dir_walk_respects_gitignore() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "build/\nignored.txt\n").expect("write gitignore");
        fs::write(root.join("kept.txt"), "keep me").expect("write kept");
        fs::write(root.join("ignored.txt"), "drop me").expect("write ignored");
        fs::create_dir(root.join("build")).expect("mkdir build");
        fs::write(root.join("build/artifact.txt"), "binary-ish").expect("write artifact");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");
        let names: Vec<_> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string().replace('\\', "/"))
            .collect();
        assert!(names.iter().any(|n| n.contains("kept.txt")));
        assert!(!names.iter().any(|n| n.contains("ignored.txt")));
        assert!(!names.iter().any(|n| n.contains("build/")));
    }

    // ── file resolution ──────────────────────────────────────────────────

    #[test]
    fn resolve_file_reads_contents_and_reports_token_cost() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let body = "fn main() {}\n".repeat(100); // 1300 bytes
        fs::write(root.join("main.rs"), &body).expect("write");

        let at = AtRef::parse("@main.rs").expect("parse");
        let payload = resolve(&at, root).expect("resolve");
        assert_eq!(payload.kind, PayloadKind::File);
        assert_eq!(payload.files.len(), 1);
        assert_eq!(payload.files[0].content, body);
        assert_eq!(payload.bytes(), body.len());
        // ~4 chars/token heuristic.
        assert_eq!(payload.tokens(), body.len().div_ceil(4));
        assert!(!payload.has_warnings());
    }

    #[test]
    fn resolve_file_missing_is_not_found() {
        let tmp = TempDir::new().expect("tempdir");
        let at = AtRef::parse("@nope.rs").expect("parse");
        let err = resolve(&at, tmp.path()).expect_err("missing file");
        assert!(matches!(err, AtRefError::NotFound(_)));
    }

    // ── dir size budget ──────────────────────────────────────────────────

    #[test]
    fn small_dir_resolves_with_full_contents_and_no_warning() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("a.txt"), "alpha").expect("write a");
        fs::write(root.join("b.txt"), "bravo").expect("write b");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve");
        assert_eq!(payload.kind, PayloadKind::Dir);
        assert_eq!(payload.files.len(), 2);
        assert!(payload.files.iter().all(|f| !f.content.is_empty()));
        assert!(!payload.has_warnings());
    }

    #[test]
    fn oversized_dir_warns_and_degrades_to_names_only() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        // Each file ~40k bytes; three of them blow the 25k-token budget
        // (~120k bytes / 4 ≈ 30k tokens).
        let big = "x".repeat(40_000);
        for i in 0..3 {
            fs::write(root.join(format!("big{i}.txt")), &big).expect("write big");
        }

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve");
        assert_eq!(payload.kind, PayloadKind::DirNamesOnly);
        // Names are kept, bodies dropped — the safe fallback by default.
        assert_eq!(payload.files.len(), 3);
        assert!(payload.files.iter().all(|f| f.content.is_empty()));
        let warned = payload.warnings.iter().any(
            |w| matches!(w, AtWarning::OversizedDir { tokens } if *tokens > DIR_TOKEN_WARN_BUDGET),
        );
        assert!(warned, "an oversized @dir must warn");
    }

    #[test]
    fn resolve_dir_missing_is_not_found() {
        let tmp = TempDir::new().expect("tempdir");
        let at = AtRef::parse("@nope/").expect("parse");
        let err = resolve(&at, tmp.path()).expect_err("missing dir");
        assert!(matches!(err, AtRefError::NotFound(_)));
    }

    // ── non-filesystem kinds ─────────────────────────────────────────────

    #[test]
    fn symbol_diff_url_session_output_resolve_to_deferred_payloads() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();

        let sym = resolve(&AtRef::parse("@MyType").unwrap(), root).unwrap();
        assert_eq!(sym.kind, PayloadKind::Symbol);
        assert!(sym.files.is_empty() && !sym.text.is_empty());

        let diff = resolve(&AtRef::parse("@diff main").unwrap(), root).unwrap();
        assert_eq!(diff.kind, PayloadKind::Diff);
        assert!(diff.text.contains("main"));

        let url = resolve(&AtRef::parse("@url https://x.io/a").unwrap(), root).unwrap();
        assert_eq!(url.kind, PayloadKind::Url);
        assert!(url.text.contains("https://x.io/a"));

        let sess = resolve(&AtRef::parse("@session s1").unwrap(), root).unwrap();
        assert_eq!(sess.kind, PayloadKind::Session);

        let out = resolve(&AtRef::parse("@output").unwrap(), root).unwrap();
        assert_eq!(out.kind, PayloadKind::Output);
    }

    // ── misc helpers ─────────────────────────────────────────────────────

    #[test]
    fn estimate_tokens_uses_the_four_char_heuristic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2); // div_ceil
    }

    #[test]
    fn rel_to_root_rejects_paths_escaping_the_root() {
        let root = Path::new("/project");
        assert_eq!(
            rel_to_root(Path::new("/project/src/x.rs"), root).as_deref(),
            Some("src/x.rs")
        );
        assert!(rel_to_root(Path::new("/elsewhere/x.rs"), root).is_none());
    }

    // ── identity guards: core#339 / core#335 / core#323 ───────────────────
    //
    // Every fixture below uses an OBVIOUSLY FAKE credential body. The point
    // is that the bytes reach the payload at all, not what they say.

    /// Deliberately non-resolvable, and obviously not a real credential.
    #[cfg(unix)]
    const FAKE_CREDENTIAL: &str = "https://fake-user:fake-token@example.invalid\n";

    /// A secret file name that was ALREADY on the denylist at the base
    /// commit. Using it keeps the identity tests independent of this
    /// change's `.git-credentials` addition: they fail before the fix and
    /// pass after it purely because of *which object* the guard judges.
    #[cfg(unix)]
    const PRE_EXISTING_SECRET_NAME: &str = ".netrc";

    /// core#339, call site `resolve_file` — `fs::read_to_string(&full)`.
    /// The guard matches the LEXICAL name (`notes.txt`); the read follows
    /// the symlink to the real target.
    #[cfg(unix)]
    #[test]
    fn at_file_through_a_symlink_must_not_launder_a_secret() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let outside = TempDir::new().expect("outside");
        let cred = outside.path().join(PRE_EXISTING_SECRET_NAME);
        fs::write(&cred, FAKE_CREDENTIAL).expect("write fake credential");
        std::os::unix::fs::symlink(&cred, root.join("notes.txt")).expect("symlink");

        let at = AtRef::parse("@notes.txt").expect("parse");
        match resolve(&at, root) {
            Err(AtRefError::SecretBlocked(_)) => {}
            Err(other) => panic!("expected SecretBlocked, got {other:?}"),
            Ok(payload) => panic!(
                "credential laundered through a symlink: {:?}",
                payload.files[0].content
            ),
        }
    }

    /// core#339, call site `walk_dir` — `fs::read_to_string(&path)`.
    #[cfg(unix)]
    #[test]
    fn at_dir_walk_must_not_inline_a_secret_reached_through_a_symlink() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let outside = TempDir::new().expect("outside");
        let cred = outside.path().join(PRE_EXISTING_SECRET_NAME);
        fs::write(&cred, FAKE_CREDENTIAL).expect("write fake credential");
        fs::write(root.join("ok.txt"), "safe").expect("write ok");
        std::os::unix::fs::symlink(&cred, root.join("notes.txt")).expect("symlink");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");
        let leaked: Vec<_> = payload
            .files
            .iter()
            .filter(|f| f.content.contains("fake-token"))
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            leaked.is_empty(),
            "@dir walk inlined a symlinked credential via {leaked:?}"
        );
    }

    /// core#339 amplification: a symlink to a DIRECTORY outside the
    /// workspace pulls that whole tree into an `@dir` of the workspace,
    /// because the walk judges the lexical path and reads the target.
    #[cfg(unix)]
    #[test]
    fn at_dir_walk_must_not_escape_the_workspace_through_a_directory_symlink() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let outside = TempDir::new().expect("outside");
        fs::write(outside.path().join("private.txt"), "outside-the-workspace")
            .expect("write outside");
        fs::write(root.join("ok.txt"), "safe").expect("write ok");
        std::os::unix::fs::symlink(outside.path(), root.join("vendor")).expect("symlink dir");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");
        let escaped: Vec<_> = payload
            .files
            .iter()
            .filter(|f| f.content.contains("outside-the-workspace"))
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            escaped.is_empty(),
            "@dir walk escaped the workspace via {escaped:?}"
        );
    }

    /// core#335: a `..` component makes `rel_to_root` return `None`, so the
    /// gitignore guard is skipped entirely for a path that is really inside
    /// the workspace.
    #[test]
    fn a_parent_dir_component_must_not_skip_the_gitignore_guard() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "secret.txt\n").expect("write gitignore");
        fs::write(root.join("secret.txt"), "ignored body").expect("write file");
        fs::create_dir(root.join("sub")).expect("mkdir sub");

        let at = AtRef::parse("@sub/../secret.txt").expect("parse");
        match resolve(&at, root) {
            Err(AtRefError::GitIgnored(_)) => {}
            Err(other) => panic!("expected GitIgnored, got {other:?}"),
            Ok(p) => panic!(
                "`..` skipped the gitignore guard: {:?}",
                p.files[0].content
            ),
        }
    }

    /// core#335: the same skip, reached the way the issue describes — an
    /// ABSOLUTE path, against a root that is spelled through a symlink (the
    /// ordinary case on macOS, where `/tmp` is `/private/tmp`).
    #[cfg(unix)]
    #[test]
    fn an_absolute_path_under_a_symlinked_root_must_not_skip_the_gitignore_guard() {
        let tmp = TempDir::new().expect("tempdir");
        let real = tmp.path().join("real");
        fs::create_dir(&real).expect("mkdir real");
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink root");
        fs::write(real.join(".gitignore"), "secret.txt\n").expect("write gitignore");
        fs::write(real.join("secret.txt"), "ignored body").expect("write file");

        let abs = real.join("secret.txt");
        let at = AtRef::parse(&format!("@{}", abs.display())).expect("parse");
        // `link` is the workspace root as the composer knows it.
        match resolve(&at, &link) {
            Err(AtRefError::GitIgnored(_)) => {}
            Err(other) => panic!("expected GitIgnored, got {other:?}"),
            Ok(p) => panic!(
                "absolute path under a symlinked root skipped the gitignore guard: {:?}",
                p.files[0].content
            ),
        }
    }

    /// core#339 as reported, verbatim: `ln -s ~/.git-credentials notes.txt`
    /// then `@notes.txt`. Needs BOTH halves of this change — the denylist
    /// entry for `.git-credentials` and the identity guard that judges the
    /// symlink's target instead of its name.
    #[cfg(unix)]
    #[test]
    fn the_reported_git_credentials_symlink_is_refused() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let home = TempDir::new().expect("home");
        let cred = home.path().join(".git-credentials");
        fs::write(&cred, FAKE_CREDENTIAL).expect("write fake credential");
        std::os::unix::fs::symlink(&cred, root.join("notes.txt")).expect("symlink");

        let err = resolve(&AtRef::parse("@notes.txt").unwrap(), root)
            .expect_err("the reported exploit must be refused");
        assert!(matches!(err, AtRefError::SecretBlocked(_)), "got {err:?}");

        // …and the `@dir` walk must not pick it up either.
        let payload = resolve(&AtRef::parse("@./").unwrap(), root).expect("resolve dir");
        assert!(
            !payload.files.iter().any(|f| f.content.contains("fake-token")),
            "@dir walk inlined the reported credential store"
        );
    }

    /// An ordinary symlink to an ordinary file must still resolve. A
    /// blanket refusal of symlinks would pass every test above and break
    /// every repo that links a real file.
    #[cfg(unix)]
    #[test]
    fn an_ordinary_symlink_to_an_ordinary_file_still_resolves() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir(root.join("src")).expect("mkdir src");
        fs::write(root.join("src/real.rs"), "fn main() {}\n").expect("write real");
        std::os::unix::fs::symlink(root.join("src/real.rs"), root.join("link.rs"))
            .expect("symlink");

        let payload = resolve(&AtRef::parse("@link.rs").unwrap(), root).expect("resolve");
        assert_eq!(payload.files[0].content, "fn main() {}\n");

        // And through the walk, from both spellings.
        let dir = resolve(&AtRef::parse("@./").unwrap(), root).expect("resolve dir");
        let names: Vec<_> = dir
            .files
            .iter()
            .map(|f| f.path.display().to_string().replace('\\', "/"))
            .collect();
        assert!(names.iter().any(|n| n == "link.rs"), "{names:?}");
        assert!(names.iter().any(|n| n == "src/real.rs"), "{names:?}");
    }

    /// core#335's capability half: an absolute path OUTSIDE the workspace is
    /// a real feature and must keep working. It gets both name gates, but
    /// this root's `.gitignore` has no jurisdiction over it.
    #[test]
    fn an_absolute_path_outside_the_workspace_still_resolves() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "notes.txt\n").expect("write gitignore");
        let outside = TempDir::new().expect("outside");
        let target = outside.path().join("notes.txt");
        fs::write(&target, "outside body").expect("write outside");

        let at = AtRef::parse(&format!("@{}", target.display())).expect("parse");
        let payload = resolve(&at, root).expect("an out-of-tree absolute path still attaches");
        assert_eq!(payload.files[0].content, "outside body");
    }

    /// A self-referential directory symlink must not multiply the
    /// workspace. Today the walk re-enters the link at every level and only
    /// stops at `DIR_MAX_FILES`.
    #[cfg(unix)]
    #[test]
    fn a_directory_symlink_cycle_must_not_multiply_the_walk() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("ok.txt"), "safe").expect("write ok");
        std::os::unix::fs::symlink(root, root.join("loop")).expect("symlink cycle");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");
        assert!(
            payload.files.len() <= 2,
            "symlink cycle multiplied the walk to {} entries",
            payload.files.len()
        );
    }
}
