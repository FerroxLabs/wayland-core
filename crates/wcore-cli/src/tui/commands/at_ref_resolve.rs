//! Resolution for `@`-references — turning a parsed [`AtRef`] into the
//! [`AtPayload`] a message carries.
//!
//! This module owns the [`AtPayload`] / [`ResolvedFile`] / [`AtWarning`]
//! types and the [`resolve`] entry point. Filesystem kinds (`@file`,
//! `@dir`) are read here under the secret + gitignore guardrails from
//! [`at_ref_guard`]; the network/engine kinds (`@url`, `@session`,
//! `@symbol`, `@diff`) resolve to deferred placeholders whose real work
//! happens behind the protocol bridge. Split out of `at_refs.rs` (W3-B).

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use super::at_ref_guard::{GitIgnore, canonical_root, is_secret_path, rel_to_root, resolve_target};
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
fn resolve_file(path: &Path, root: &Path) -> Result<AtPayload, AtRefError> {
    let full = resolve_under_root(path, root);
    let ignore = GitIgnore::load(root);

    // Lexical pass, on the name as typed. Cheap, and it keeps `@.env` a
    // loud refusal even when no such file exists — a denylisted NAME is
    // refused without the filesystem being touched at all.
    if is_secret_path(&full) {
        return Err(AtRefError::SecretBlocked(display(path)));
    }
    if let Some(rel) = rel_to_root(&full, root)
        && ignore.is_ignored(&rel, false)
    {
        return Err(AtRefError::GitIgnored(display(path)));
    }

    // Identity pass. `full` is still only a name, and the read follows it.
    // Resolve it once to the object that read will consume, then judge THAT
    // object: a symlink to `~/.git-credentials` is refused here, a symlink
    // to an ordinary file is not (core#339). Measuring the canonical path
    // against the canonical root also gives an absolute or `..`-routed
    // target the gitignore verdict the lexical pass could not compute,
    // because `strip_prefix` could not put the two in the same form
    // (core#335).
    let target = resolve_target(&full).map_err(|e| target_error(&e, path))?;
    if is_secret_path(target.canonical()) {
        return Err(AtRefError::SecretBlocked(display(path)));
    }
    if let Some(rel) = rel_to_root(target.canonical(), &canonical_root(root))
        && ignore.is_ignored(&rel, false)
    {
        return Err(AtRefError::GitIgnored(display(path)));
    }

    let content = target.read_to_string().map_err(|e| AtRefError::Io {
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
    // The walk ROOT has to be inside the workspace, and `full` is still
    // only a name: `@link/`, where `link` is a symlink to a directory
    // outside the root, satisfies `is_dir` and then hands `walk_dir` an
    // out-of-tree tree whose every entry strips back to `link/…` — in-root
    // -looking paths for files the workspace does not contain.
    let croot = canonical_root(root);
    match fs::canonicalize(&full) {
        Ok(canonical) if canonical.starts_with(&croot) => {}
        _ => return Err(AtRefError::NotFound(display(path))),
    }

    let ignore = GitIgnore::load(root);
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped = 0usize;
    let mut truncated = false;

    walk_dir(
        &full,
        root,
        &ignore,
        &mut files,
        &mut skipped,
        &mut truncated,
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
fn walk_dir(
    dir: &Path,
    root: &Path,
    ignore: &GitIgnore,
    out: &mut Vec<ResolvedFile>,
    skipped: &mut usize,
    truncated: &mut bool,
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
        // `symlink_metadata`, NOT `Path::is_dir`, which follows symlinks:
        // a symlink to a directory took the recurse branch below — the one
        // branch with no identity guard on it — while `rel_to_root` kept
        // answering `link/…` for every entry underneath. The walk left the
        // workspace and every path it reported still looked in-root.
        //
        // A symlink therefore reports `is_dir == false` here and falls to
        // the file branch, where `resolve_target` judges the object it
        // opens and refuses anything that is not a regular file. That
        // refuses a symlink to a DIRECTORY, deliberately: following one
        // would need a containment test *and* cycle bookkeeping, because
        // `root/loop -> root` is inside the root and the `DIR_MAX_FILES`
        // budget cannot stop a cycle — each lap re-pushes the same files
        // under a longer prefix. A symlink to a FILE is still followed,
        // which is the capability repositories actually use, and the skip
        // is reported in the payload's `SkippedFiles` count rather than
        // being silent.
        let Ok(meta) = fs::symlink_metadata(&path) else {
            *skipped += 1;
            continue;
        };
        let is_dir = meta.is_dir();
        let rel = match rel_to_root(&path, root) {
            Some(r) => r,
            None => continue,
        };
        if ignore.is_ignored(&rel, is_dir) {
            *skipped += 1;
            continue;
        }
        if is_dir {
            // `.git` is always skipped — it is never useful context and
            // can be enormous.
            if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            walk_dir(&path, root, ignore, out, skipped, truncated)?;
        } else {
            // Lexical pass first: a denylisted NAME never needs opening.
            if is_secret_path(&path) {
                *skipped += 1;
                continue;
            }
            // Then the identity pass. The second of the guard's three
            // production call sites: without this the walk inlines whatever
            // an entry points at, so a fix applied only to `resolve_file`
            // would leave `@dir` wide open (core#339).
            //
            // A target that will not resolve — a dangling link, a device, a
            // file that cannot be opened — is skipped, exactly as an
            // unreadable file was before.
            let Ok(target) = resolve_target(&path) else {
                *skipped += 1;
                continue;
            };
            if is_secret_path(target.canonical()) {
                *skipped += 1;
                continue;
            }
            // Read text files only; a binary file is skipped silently
            // rather than corrupting the payload with lossy bytes.
            match target.read_to_string() {
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

/// Map a target-resolution failure onto the resolver's error vocabulary.
///
/// A missing path and a path that is not a regular file are both "there is
/// no file here" to the composer, and that is what the previous
/// `!full.is_file()` check reported for either. Anything else — a
/// permission failure, an identity that moved mid-resolution — is a real
/// I/O refusal and is surfaced as one rather than disguised as absence.
fn target_error(e: &std::io::Error, path: &Path) -> AtRefError {
    match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
            AtRefError::NotFound(display(path))
        }
        _ => AtRefError::Io {
            path: display(path),
            message: e.to_string(),
        },
    }
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

    /// `resolve_file` must refuse a credential store the FILE TOOLS already
    /// refuse to read. `.git-credentials` is a plaintext
    /// `https://user:token@host` store on `wcore-tools`' denylist; the
    /// `@`-attach guard carried its own, shorter list and inlined the file
    /// into the outgoing prompt verbatim.
    #[test]
    fn resolving_a_git_credentials_file_is_a_loud_error() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join(".git-credentials"),
            "https://fake-user:fake-token@example.invalid\n",
        )
        .expect("write fixture");

        let at = AtRef::parse("@.git-credentials").expect("parse");
        let err = resolve(&at, root).expect_err("must refuse a credential store");
        assert!(matches!(err, AtRefError::SecretBlocked(_)), "got {err:?}");
    }

    /// The other production call site of the same guard: the `@dir` walk.
    /// A guard fixed only in `resolve_file` would leave this path open.
    #[test]
    fn an_at_dir_walk_skips_a_workspace_policy_secret() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("ok.txt"), "safe").expect("write ok");
        fs::write(
            root.join(".git-credentials"),
            "https://fake-user:fake-token@example.invalid\n",
        )
        .expect("write fixture");
        fs::write(root.join("terraform.tfstate"), "{}").expect("write fixture");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");
        let names: Vec<String> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        // Control: the walk did produce output, so the two refutations below
        // cannot pass by returning nothing.
        assert!(names.iter().any(|n| n.contains("ok.txt")), "{names:?}");
        assert!(
            !names.iter().any(|n| n.contains(".git-credentials")),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("terraform.tfstate")),
            "{names:?}"
        );
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

    // ── target identity: symlinks and routes (core#339 / core#335) ───────

    /// An obviously-fake credential-store body. It is a SHAPE, not a
    /// secret — the tests below assert it never reaches a payload, so the
    /// fixture must be safe to print in a failure message.
    #[cfg(unix)]
    const FAKE_CREDENTIAL_BODY: &str = "https://fake-user:fake-token@example.invalid\n";

    /// Plant `<root>/<link>` as a symlink to `<outside>/<target>`, which
    /// holds the fake credential body. Returns nothing — every caller
    /// asserts on the payload, not on the fixture.
    #[cfg(unix)]
    fn plant_credential_symlink(root: &Path, outside: &Path, link: &str, target: &str) {
        let target = outside.join(target);
        fs::write(&target, FAKE_CREDENTIAL_BODY).expect("write fixture");
        std::os::unix::fs::symlink(&target, root.join(link)).expect("symlink");
    }

    /// A workspace + an out-of-tree "home" holding the credential store.
    #[cfg(unix)]
    fn workspace_and_outside(tmp: &TempDir) -> (PathBuf, PathBuf) {
        let root = tmp.path().join("ws");
        let outside = tmp.path().join("home");
        fs::create_dir_all(&root).expect("mkdir ws");
        fs::create_dir_all(&outside).expect("mkdir home");
        (root, outside)
    }

    /// core#339, production call site 1 of 3 — `resolve_file`.
    ///
    /// Every guard in `at_ref_guard` judges a NAME. `notes.txt` is not on
    /// any denylist, so it passed; `fs::read_to_string` then followed the
    /// link and inlined the credential store into the outgoing prompt.
    #[cfg(unix)]
    #[test]
    fn an_at_file_symlinked_to_a_credential_store_is_refused() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        plant_credential_symlink(&root, &outside, "notes.txt", ".git-credentials");
        fs::write(root.join("real.txt"), "ordinary").expect("write control");

        // Control: an ordinary file in the same workspace still resolves, so
        // a refusal below cannot come from a resolver that refuses anything.
        let ok = resolve(&AtRef::parse("@real.txt").expect("parse"), &root).expect("control");
        assert_eq!(ok.files[0].content, "ordinary");

        match resolve(&AtRef::parse("@notes.txt").expect("parse"), &root) {
            Err(AtRefError::SecretBlocked(_)) => {}
            Ok(p) => {
                let inlined = p.files.iter().any(|f| f.content.contains("fake-token"));
                panic!("a symlink to a credential store resolved (body inlined: {inlined})");
            }
            Err(other) => panic!("expected SecretBlocked, got {other:?}"),
        }
    }

    /// The same defect through the file-NAME half of the union rather than
    /// the path-fragment half, so a fix that only reaches one list is caught.
    #[cfg(unix)]
    #[test]
    fn an_at_file_symlinked_to_a_private_key_is_refused() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        plant_credential_symlink(&root, &outside, "notes.txt", "id_rsa");

        let err = resolve(&AtRef::parse("@notes.txt").expect("parse"), &root)
            .expect_err("a symlink to a private key must be refused");
        assert!(matches!(err, AtRefError::SecretBlocked(_)), "got {err:?}");
    }

    /// core#339, production call site 2 of 3 — the `@dir` walk. A fix
    /// applied only in `resolve_file` leaves this path wide open.
    #[cfg(unix)]
    #[test]
    fn an_at_dir_walk_refuses_a_symlink_to_a_credential_store() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        plant_credential_symlink(&root, &outside, "notes.txt", ".git-credentials");
        fs::write(root.join("ok.txt"), "safe").expect("write ok");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("resolve dir");
        // Control: the walk produced output, so the refutation below cannot
        // pass by returning an empty payload.
        assert!(
            payload.files.iter().any(|f| f.content == "safe"),
            "the walk produced nothing"
        );
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("fake-token")),
            "the @dir walk inlined a symlinked credential store"
        );
    }

    /// A symlink to an ordinary file is NOT refused. Repositories legitimately
    /// symlink real files; a guard that blanket-refuses symlinks removes a
    /// capability and gets routed around, which is worse than the leak.
    #[cfg(unix)]
    #[test]
    fn a_symlink_to_an_ordinary_file_still_resolves() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        fs::write(outside.join("shared.md"), "shared body").expect("write");
        std::os::unix::fs::symlink(outside.join("shared.md"), root.join("link.md"))
            .expect("symlink");

        let payload = resolve(&AtRef::parse("@link.md").expect("parse"), &root)
            .expect("an ordinary symlink must still resolve");
        assert_eq!(payload.files[0].content, "shared body");
    }

    /// core#335: an ABSOLUTE path was taken as-is, so `rel_to_root` could not
    /// strip it against a workspace root reached through a symlink and the
    /// gitignore check was skipped entirely.
    #[cfg(unix)]
    #[test]
    fn a_gitignored_file_named_by_absolute_path_under_a_symlinked_root_is_refused() {
        let tmp = TempDir::new().expect("tempdir");
        let real = tmp.path().join("real");
        fs::create_dir_all(&real).expect("mkdir real");
        fs::write(real.join(".gitignore"), "ignored.txt\n").expect("write gitignore");
        fs::write(real.join("ignored.txt"), "ignored body").expect("write ignored");
        fs::write(real.join("kept.txt"), "kept body").expect("write kept");
        let root = tmp.path().join("ws");
        std::os::unix::fs::symlink(&real, &root).expect("symlink root");

        // Control: a NON-ignored file named the same absolute way resolves,
        // so the refusal below is the gitignore rule and not the route.
        let kept = AtRef::parse(&format!("@{}", real.join("kept.txt").display())).expect("parse");
        assert_eq!(
            resolve(&kept, &root).expect("control").files[0].content,
            "kept body"
        );

        let at = AtRef::parse(&format!("@{}", real.join("ignored.txt").display())).expect("parse");
        let err = resolve(&at, &root).expect_err("a git-ignored file must be refused");
        assert!(matches!(err, AtRefError::GitIgnored(_)), "got {err:?}");
    }

    /// core#335, the same skip reached without a symlink: a `..` that climbs
    /// and comes back makes `rel_to_root` bail on the residual `ParentDir`.
    #[test]
    fn a_gitignored_file_reached_through_a_parent_traversal_is_refused() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir(root.join("sub")).expect("mkdir sub");
        fs::write(root.join(".gitignore"), "ignored.txt\n").expect("write gitignore");
        fs::write(root.join("sub/ignored.txt"), "ignored body").expect("write ignored");
        fs::write(root.join("sub/kept.txt"), "kept body").expect("write kept");

        // Control: the same route to a non-ignored sibling still resolves.
        let kept = AtRef::parse("@sub/../sub/kept.txt").expect("parse");
        assert_eq!(
            resolve(&kept, root).expect("control").files[0].content,
            "kept body"
        );

        let at = AtRef::parse("@sub/../sub/ignored.txt").expect("parse");
        let err = resolve(&at, root).expect_err("a git-ignored file must be refused");
        assert!(matches!(err, AtRefError::GitIgnored(_)), "got {err:?}");
    }

    // ── @dir walk confinement (core#339 follow-up) ───────────────────────

    /// The `@dir` walk was never root-confined.
    ///
    /// `Path::is_dir` FOLLOWS symlinks, so a symlink to a directory took
    /// the recurse branch — the one with no identity guard at all — and
    /// `rel_to_root` then answered `link/…` for every entry underneath.
    /// The walk left the workspace while every path it reported looked
    /// in-root.
    #[cfg(unix)]
    #[test]
    fn an_at_dir_walk_does_not_follow_a_symlink_to_a_directory_outside_the_root() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        let private = outside.join("private");
        fs::create_dir_all(&private).expect("mkdir private");
        fs::write(private.join("leak.txt"), FAKE_CREDENTIAL_BODY).expect("write leak");
        std::os::unix::fs::symlink(&private, root.join("link")).expect("symlink dir");
        fs::create_dir(root.join("sub")).expect("mkdir sub");
        fs::write(root.join("sub/kept.txt"), "kept body").expect("write kept");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("resolve dir");
        // Control: an ordinary sub-directory IS walked, so the refutations
        // below cannot pass by walking nothing.
        assert!(
            payload.files.iter().any(|f| f.content == "kept body"),
            "the walk skipped an ordinary sub-directory: {:?}",
            payload.files
        );
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("fake-token")),
            "the @dir walk inlined a file from outside the workspace"
        );
        assert!(
            !payload.files.iter().any(|f| f.path.starts_with("link")),
            "the walk descended through a symlinked directory: {:?}",
            payload.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }

    /// The walk ROOT has to be confined too. `@link/`, where `link` points
    /// at a directory outside the workspace, satisfies `is_dir` and then
    /// hands `walk_dir` an out-of-tree tree whose every entry strips to
    /// `link/…` against the root.
    #[cfg(unix)]
    #[test]
    fn an_at_dir_reference_to_a_symlinked_directory_outside_the_root_is_refused() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        let private = outside.join("private");
        fs::create_dir_all(&private).expect("mkdir private");
        fs::write(private.join("leak.txt"), FAKE_CREDENTIAL_BODY).expect("write leak");
        std::os::unix::fs::symlink(&private, root.join("link")).expect("symlink dir");
        fs::create_dir(root.join("sub")).expect("mkdir sub");
        fs::write(root.join("sub/kept.txt"), "kept body").expect("write kept");

        // Control: an ordinary in-root directory still resolves.
        let ok = resolve(&AtRef::parse("@sub/").expect("parse"), &root).expect("control");
        assert!(
            ok.files.iter().any(|f| f.content == "kept body"),
            "the control directory produced nothing"
        );

        match resolve(&AtRef::parse("@link/").expect("parse"), &root) {
            Err(AtRefError::NotFound(_)) => {}
            Ok(p) => panic!(
                "@dir walked outside the workspace: {:?}",
                p.files.iter().map(|f| &f.path).collect::<Vec<_>>()
            ),
            Err(other) => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// A symlink cycle must terminate exactly once. `root/loop -> root` is
    /// INSIDE the root, so a containment test alone does not stop it: the
    /// `DIR_MAX_FILES` budget only trips when files are pushed, and each
    /// lap pushes the same file again under a longer `loop/…/` prefix.
    #[cfg(unix)]
    #[test]
    fn an_at_dir_walk_terminates_on_a_symlink_cycle() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, _outside) = workspace_and_outside(&tmp);
        fs::write(root.join("kept.txt"), "kept body").expect("write kept");
        std::os::unix::fs::symlink(&root, root.join("loop")).expect("symlink cycle");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("resolve dir");
        assert!(
            payload.files.iter().any(|f| f.content == "kept body"),
            "the walk produced nothing"
        );
        assert_eq!(
            payload
                .files
                .iter()
                .filter(|f| f.content == "kept body")
                .count(),
            1,
            "the cycle was walked more than once: {:?}",
            payload.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }
}
