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
use std::path::{Path, PathBuf};

use super::at_ref_guard::{AtGate, GitIgnore, Reach, Refusal, is_dir_following_links, rel_to_root};
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
    /// One or more FILES in an `@dir` walk were skipped because they are
    /// git-ignored, secret, unreadable, or resolve outside the workspace.
    /// Carries the count for an honest "N skipped".
    SkippedFiles {
        /// How many files the walk skipped.
        count: usize,
    },
    /// One or more DIRECTORIES in an `@dir` walk were not descended into,
    /// for the same reasons.
    ///
    /// Counted apart from [`AtWarning::SkippedFiles`] because a directory is
    /// not a file: reporting a skipped subtree as "1 file skipped" tells the
    /// user a number they cannot reconcile with what is missing, which is
    /// how a removed capability passed for a routine skip.
    SkippedDirs {
        /// How many directories the walk did not descend into.
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
                write!(
                    f,
                    "{count} file(s) skipped (git-ignored, secret, or outside the workspace)"
                )
            }
            AtWarning::SkippedDirs { count } => {
                write!(
                    f,
                    "{count} director(y/ies) not walked (git-ignored, secret, or outside the workspace)"
                )
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

/// Resolve `@file`: read one named file through the shared admission gate.
fn resolve_file(path: &Path, root: &Path) -> Result<AtPayload, AtRefError> {
    let full = resolve_under_root(path, root);
    let ignore = GitIgnore::load(root);
    let gate = AtGate::new(root, &ignore);

    // Every rule — the secret denylist on both the typed and the canonical
    // name, `.gitignore` on both, and regular-file identity taken from the
    // handle the read will consume — lives in the gate. This function no
    // longer decides any of them; it only says which reach applies.
    //
    // `Reach::Named` is the one deliberate difference between the consumers:
    // the payload carries this file under the name the user typed, so a link
    // that leaves the workspace is the capability repositories use rather
    // than a substituted identity. See [`Reach`].
    let target = gate
        .admit_file(&full, Reach::Named)
        .map_err(|r| refusal_error(r, path))?;

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

/// Resolve `@dir`: walk a directory tree through the same gate every other
/// consumer uses. An oversized tree resolves with an
/// [`AtWarning::OversizedDir`] so the composer can offer names-only.
fn resolve_dir(path: &Path, root: &Path) -> Result<AtPayload, AtRefError> {
    let full = resolve_under_root(path, root);
    if !full.is_dir() {
        return Err(AtRefError::NotFound(display(path)));
    }

    let ignore = GitIgnore::load(root);
    let gate = AtGate::new(root, &ignore);

    // The walk ROOT goes through the gate exactly like every entry beneath
    // it. `@link/`, where `link` resolves outside the workspace, otherwise
    // hands the walk an out-of-tree tree whose every entry strips back to
    // `link/…` — in-root-looking paths for files the workspace does not
    // contain.
    let canonical_dir = gate.admit_dir(&full).map_err(|r| refusal_error(r, path))?;

    let mut walk = DirWalk::default();
    walk.visited.insert(canonical_dir);
    walk.run(&full, &gate)?;

    let mut warnings = Vec::new();
    if walk.truncated {
        warnings.push(AtWarning::Truncated {
            limit: DIR_MAX_FILES,
        });
    }
    if walk.skipped_files > 0 {
        warnings.push(AtWarning::SkippedFiles {
            count: walk.skipped_files,
        });
    }
    if walk.skipped_dirs > 0 {
        warnings.push(AtWarning::SkippedDirs {
            count: walk.skipped_dirs,
        });
    }

    let files = walk.files;
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

/// The mutable state of one `@dir` walk.
///
/// A struct rather than six `&mut` out-parameters, because the visited set
/// is the fifth and the two skip counters have to stay distinguishable.
#[derive(Default)]
struct DirWalk {
    /// Files admitted so far, labelled relative to the workspace root.
    files: Vec<ResolvedFile>,
    /// CANONICAL paths of directories already walked.
    ///
    /// Keyed on the canonical path, so one directory is walked once however
    /// many names reach it. That is what makes following in-root directory
    /// symlinks safe: `root/loop -> root` stops on the first repeat instead
    /// of re-pushing the whole tree under an ever-longer prefix — the cycle
    /// that `DIR_MAX_FILES` could bound only by truncating the payload.
    visited: HashSet<PathBuf>,
    /// Files the gate refused, or that could not be read as text.
    skipped_files: usize,
    /// Directories the gate refused. Kept apart from `skipped_files` so the
    /// user-facing warning does not call a subtree a file.
    skipped_dirs: usize,
    /// True once `DIR_MAX_FILES` was hit.
    truncated: bool,
}

impl DirWalk {
    /// Depth-first walk of `dir`, admitting every candidate through `gate`.
    fn run(&mut self, dir: &Path, gate: &AtGate) -> Result<(), AtRefError> {
        if self.truncated {
            return Ok(());
        }
        let entries = fs::read_dir(dir).map_err(|e| AtRefError::Io {
            path: display(dir),
            message: e.to_string(),
        })?;

        // Sort entries for a deterministic walk — the payload (and its
        // tests) must not depend on filesystem iteration order, and with a
        // visited set the order also decides which of two names for the
        // same directory is the one the payload reports.
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();

        for path in paths {
            if self.files.len() >= DIR_MAX_FILES {
                self.truncated = true;
                return Ok(());
            }

            // Decide what the entry IS by following the link; let the gate
            // decide whether it may be entered. Classifying with
            // `symlink_metadata` (or `DirEntry::file_type`, the same stat)
            // calls every directory link a non-directory, which is how
            // `@alias/` resolved while `@./` silently dropped the same
            // object and reported it as a skipped FILE.
            if is_dir_following_links(&path) {
                // `.git` is never useful context and can be enormous. Not a
                // refusal, so it is not counted as one.
                if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                    continue;
                }
                match gate.admit_dir(&path) {
                    // Already reached under another name: the same object,
                    // not a refusal, so it is de-duplicated silently rather
                    // than reported as a skip.
                    Ok(canonical) => {
                        if self.visited.insert(canonical) {
                            self.run(&path, gate)?;
                        }
                    }
                    Err(_) => self.skipped_dirs += 1,
                }
                continue;
            }

            // The label the payload will carry. A path that will not strip
            // against the root cannot be labelled honestly, so it is not
            // carried at all.
            let Some(rel) = rel_to_root(&path, gate.root()) else {
                continue;
            };

            // `Reach::Walked`: nobody named this file, and it is about to be
            // labelled with an in-root relative path, so the object it
            // resolves to has to be in the workspace too.
            match gate.admit_file(&path, Reach::Walked) {
                Ok(target) => match target.read_to_string() {
                    Ok(content) => self.files.push(ResolvedFile {
                        path: PathBuf::from(&rel),
                        content,
                    }),
                    // A binary file is skipped rather than corrupting the
                    // payload with lossy bytes.
                    Err(_) => self.skipped_files += 1,
                },
                Err(_) => self.skipped_files += 1,
            }
        }
        Ok(())
    }
}

/// Map a gate [`Refusal`] onto the resolver's error vocabulary.
fn refusal_error(refusal: Refusal, path: &Path) -> AtRefError {
    match refusal {
        Refusal::Secret => AtRefError::SecretBlocked(display(path)),
        Refusal::GitIgnored => AtRefError::GitIgnored(display(path)),
        // "there is nothing here you may attach" — the same answer the
        // previous `is_dir` / `is_file` pre-checks gave for a target that
        // is not attachable, and what
        // `an_at_dir_reference_to_a_symlinked_directory_outside_the_root_is_refused`
        // pins.
        Refusal::EscapesRoot => AtRefError::NotFound(display(path)),
        Refusal::Unresolvable(e) => target_error(&e, path),
    }
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

    // ══ the gate: one policy, three consumers ════════════════════════════

    /// THE structural test. Round 1's failure was not three oversights but
    /// one duplication: `resolve_file`, the popup and `walk_dir` each held a
    /// private copy of the same four rules, two were tightened and the third
    /// drifted. A per-defect test cannot catch that — the next consumer to
    /// drift has no test yet. This one asserts the PROPERTY instead: for the
    /// same planted object, `@name` and `@./` reach the same verdict.
    ///
    /// One row disagrees ON PURPOSE, and says so: an in-root name pointing
    /// at an ordinary file OUTSIDE the workspace resolves as `@name` (the
    /// user named it; `a_symlink_to_an_ordinary_file_still_resolves` pins
    /// that capability) and is refused by the walk (nobody named it, and the
    /// walk would label its body with an in-root path). That is `Reach`, and
    /// it is the only difference the gate permits.
    #[cfg(unix)]
    #[test]
    fn the_walk_and_the_direct_reference_reach_the_same_verdict() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        fs::write(root.join(".gitignore"), "artifact.txt\n").expect("gitignore");
        fs::write(root.join("artifact.txt"), "IGNORED-BODY").expect("artifact");
        fs::write(root.join("ordinary.txt"), "ordinary body").expect("ordinary");
        fs::write(outside.join("shared.md"), "OUTSIDE-BODY").expect("shared");
        std::os::unix::fs::symlink(outside.join("shared.md"), root.join("outside_link.txt"))
            .expect("outside link");
        std::os::unix::fs::symlink(root.join("artifact.txt"), root.join("ignored_link.txt"))
            .expect("ignored link");
        plant_credential_symlink(&root, &outside, "secret_link.txt", ".git-credentials");

        // (name, direct `@name` resolves?, appears in the `@./` walk?)
        let cases: &[(&str, bool, bool)] = &[
            ("ordinary.txt", true, true),
            ("artifact.txt", false, false),
            ("ignored_link.txt", false, false),
            ("secret_link.txt", false, false),
            // The one named difference — see the doc comment above.
            ("outside_link.txt", true, false),
        ];

        let walked = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("walk");
        let names: Vec<String> = walked
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        // Control: the walk produced something, so a "not present" assertion
        // below cannot pass by the walk having produced nothing at all.
        assert!(
            names.iter().any(|n| n == "ordinary.txt"),
            "control missing, the walk produced {names:?}"
        );

        for (name, direct_ok, in_walk) in cases {
            let direct = resolve(&AtRef::parse(&format!("@{name}")).expect("parse"), &root);
            assert_eq!(
                direct.is_ok(),
                *direct_ok,
                "@{name}: direct verdict wrong, got {direct:?}"
            );
            assert_eq!(
                names.iter().any(|n| n == name),
                *in_walk,
                "@{name}: walk verdict wrong, walk produced {names:?}"
            );
        }

        // And no body from outside the workspace reached the walk at all,
        // by content rather than by name.
        for f in &walked.files {
            assert!(
                !f.content.contains("OUTSIDE-BODY") && !f.content.contains("fake-token"),
                "the walk inlined an out-of-workspace body as {:?}",
                f.path
            );
        }
    }

    /// D3, the walk half. An in-root symlink to an in-root directory is
    /// followed, and the visited set keyed on the canonical path means the
    /// object is walked ONCE however many names reach it.
    #[cfg(unix)]
    #[test]
    fn an_in_root_symlinked_directory_is_walked_exactly_once() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, _outside) = workspace_and_outside(&tmp);
        fs::create_dir(root.join("real")).expect("mkdir real");
        fs::write(root.join("real/inner.txt"), "inner body").expect("inner");
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("alias");
        fs::write(root.join("kept.txt"), "kept body").expect("kept");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("walk");
        assert_eq!(
            payload
                .files
                .iter()
                .filter(|f| f.content == "inner body")
                .count(),
            1,
            "the aliased directory was walked twice: {:?}",
            payload.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        // De-duplication is not a refusal, so it raises no advisory at all.
        assert!(
            payload.warnings.is_empty(),
            "de-duplicating one object reported as a skip: {:?}",
            payload.warnings
        );
    }

    /// D3, the honesty half. A directory the walk would not enter must be
    /// reported as a DIRECTORY. Round 1 removed the ability to follow a
    /// directory link and reported the loss as `SkippedFiles { count: 1 }` —
    /// a number the user cannot reconcile with what is missing.
    #[cfg(unix)]
    #[test]
    fn a_directory_the_walk_refuses_is_not_reported_as_a_skipped_file() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        let private = outside.join("private");
        fs::create_dir_all(&private).expect("mkdir private");
        fs::write(private.join("leak.txt"), FAKE_CREDENTIAL_BODY).expect("leak");
        std::os::unix::fs::symlink(&private, root.join("link")).expect("dir link");
        fs::write(root.join("kept.txt"), "kept body").expect("kept");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("walk");
        assert!(
            payload.files.iter().any(|f| f.content == "kept body"),
            "control missing"
        );
        assert!(
            payload
                .warnings
                .contains(&AtWarning::SkippedDirs { count: 1 }),
            "the refused directory was not reported as a directory: {:?}",
            payload.warnings
        );
        assert!(
            !payload
                .warnings
                .iter()
                .any(|w| matches!(w, AtWarning::SkippedFiles { .. })),
            "a refused DIRECTORY was counted as a skipped file: {:?}",
            payload.warnings
        );
        assert!(
            AtWarning::SkippedDirs { count: 1 }
                .to_string()
                .contains("director"),
            "the rendered advisory does not say 'directory'"
        );
    }

    /// A consistency the unified gate buys: `@build/` on a git-ignored
    /// directory is now refused, exactly as `@build/artifact.txt` was and as
    /// the completion popup already declined to offer it. Before the gate,
    /// the walk root was the one candidate no `.gitignore` verdict reached,
    /// so `@build/` inlined a whole ignored tree the popup would not name.
    #[test]
    fn an_at_dir_reference_to_a_gitignored_directory_is_refused() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "build/\n").expect("gitignore");
        fs::create_dir(root.join("build")).expect("mkdir build");
        fs::write(root.join("build/artifact.txt"), "binary-ish").expect("artifact");
        fs::create_dir(root.join("src")).expect("mkdir src");
        fs::write(root.join("src/main.rs"), "fn main() {}").expect("main");

        // Control: a non-ignored directory still resolves by the same route.
        let ok = resolve(&AtRef::parse("@src/").expect("parse"), root).expect("control");
        assert!(ok.files.iter().any(|f| f.content == "fn main() {}"));

        let err = resolve(&AtRef::parse("@build/").expect("parse"), root)
            .expect_err("a git-ignored directory must be refused");
        assert!(matches!(err, AtRefError::GitIgnored(_)), "got {err:?}");
    }

    // == REFUTATION PROBES (external auditor) =============================

    /// The @dir walk still leaves the workspace - through a symlink to a
    /// FILE. The fix closed only the DIRECTORY half.
    #[cfg(unix)]
    #[test]
    fn refut_at_dir_walk_inlines_a_file_symlinked_outside_the_root() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        let private = outside.join("private");
        fs::create_dir_all(&private).expect("mkdir private");
        fs::write(private.join("diary.txt"), "OUTSIDE-BODY-MARKER").expect("write diary");
        std::os::unix::fs::symlink(private.join("diary.txt"), root.join("notes.txt"))
            .expect("symlink file");
        fs::write(root.join("kept.txt"), "kept body").expect("write kept");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("resolve dir");
        assert!(
            payload.files.iter().any(|f| f.content == "kept body"),
            "control missing: the walk produced nothing in-root"
        );
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("OUTSIDE-BODY-MARKER")),
            "REFUTED: @dir inlined an out-of-workspace file via a FILE symlink: {:?}",
            payload
                .files
                .iter()
                .map(|f| (f.path.display().to_string(), f.content.clone()))
                .collect::<Vec<_>>()
        );
    }

    /// The @dir walk never applies the CANONICAL gitignore verdict that
    /// resolve_file applies, so the walk inlines a body the direct
    /// reference refuses by name.
    #[cfg(unix)]
    #[test]
    fn refut_at_dir_walk_honors_the_canonical_gitignore_name() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "artifact.txt\n").expect("gitignore");
        fs::write(root.join("artifact.txt"), "IGNORED-BODY-MARKER").expect("write artifact");
        std::os::unix::fs::symlink(root.join("artifact.txt"), root.join("notable.txt"))
            .expect("symlink");
        fs::write(root.join("kept.txt"), "kept body").expect("write kept");

        let direct = resolve(&AtRef::parse("@notable.txt").expect("parse"), root);
        let walk = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        assert!(
            walk.files.iter().any(|f| f.content == "kept body"),
            "control missing"
        );
        assert!(
            !walk
                .files
                .iter()
                .any(|f| f.content.contains("IGNORED-BODY-MARKER")),
            "REFUTED: @dir inlined a body the direct reference refuses ({:?}): {:?}",
            direct.as_ref().err(),
            walk.files
                .iter()
                .map(|f| f.path.display().to_string())
                .collect::<Vec<_>>()
        );
    }

    /// Polarity control: the symlinked-credential case IS covered.
    #[cfg(unix)]
    #[test]
    fn refut_control_at_dir_walk_still_refuses_a_symlinked_credential_store() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, outside) = workspace_and_outside(&tmp);
        plant_credential_symlink(&root, &outside, "notes.txt", ".git-credentials");
        fs::write(root.join("kept.txt"), "kept body").expect("write kept");
        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("resolve dir");
        assert!(
            payload.files.iter().any(|f| f.content == "kept body"),
            "control missing"
        );
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("fake-token")),
            "the credential symlink was inlined: {:?}",
            payload
                .files
                .iter()
                .map(|f| f.path.display().to_string())
                .collect::<Vec<_>>()
        );
    }

    /// An IN-ROOT symlink to a directory is no longer walked.
    #[cfg(unix)]
    #[test]
    fn refut_an_in_root_symlinked_directory_is_no_longer_walked() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, _outside) = workspace_and_outside(&tmp);
        fs::create_dir(root.join("real")).expect("mkdir real");
        fs::write(root.join("real/inner.txt"), "inner body").expect("write inner");
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("symlink dir");
        fs::write(root.join("kept.txt"), "kept body").expect("write kept");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("resolve dir");
        let names: Vec<String> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        eprintln!(
            "REFUT in-root dir symlink: names={names:?} warnings={:?}",
            payload.warnings
        );
        assert!(
            names.iter().any(|n| n.contains("kept.txt")),
            "control missing"
        );
        assert!(
            names.iter().any(|n| n.starts_with("alias")),
            "REFUTED-AS-REGRESSION: an in-root symlinked directory produced no entries: {names:?}"
        );
    }

    /// @alias/ where alias is an IN-ROOT symlink to an in-root directory.
    #[cfg(unix)]
    #[test]
    fn refut_at_dir_on_an_in_root_symlinked_directory_still_resolves() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, _outside) = workspace_and_outside(&tmp);
        fs::create_dir(root.join("real")).expect("mkdir real");
        fs::write(root.join("real/inner.txt"), "inner body").expect("write inner");
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("symlink dir");
        let got = resolve(&AtRef::parse("@alias/").expect("parse"), &root);
        eprintln!("REFUT @alias/ -> {got:?}");
        let p = got.expect("REFUTED: @alias/ on an in-root symlinked dir was refused");
        assert!(
            p.files.iter().any(|f| f.content == "inner body"),
            "@alias/ produced nothing: {:?}",
            p.files
                .iter()
                .map(|f| f.path.display().to_string())
                .collect::<Vec<_>>()
        );
    }

    /// A mutual (dangling) symlink loop must terminate.
    #[cfg(unix)]
    #[test]
    fn refut_at_dir_walk_terminates_on_a_mutual_symlink_loop() {
        let tmp = TempDir::new().expect("tempdir");
        let (root, _outside) = workspace_and_outside(&tmp);
        std::os::unix::fs::symlink(root.join("b"), root.join("a")).expect("a->b");
        std::os::unix::fs::symlink(root.join("a"), root.join("b")).expect("b->a");
        fs::write(root.join("kept.txt"), "kept body").expect("write kept");
        let payload = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("resolve dir");
        assert_eq!(
            payload
                .files
                .iter()
                .filter(|f| f.content == "kept body")
                .count(),
            1,
            "mutual loop mis-walked: {:?}",
            payload
                .files
                .iter()
                .map(|f| f.path.display().to_string())
                .collect::<Vec<_>>()
        );
    }

    /// A FIFO planted in a walked tree must not wedge the walk.
    #[cfg(unix)]
    #[test]
    fn refut_at_dir_walk_does_not_block_on_a_fifo_in_the_tree() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = TempDir::new().expect("tempdir");
        let (root, _outside) = workspace_and_outside(&tmp);
        fs::write(root.join("kept.txt"), "kept body").expect("write kept");
        let fifo = root.join("pipe");
        let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).expect("cstr");
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");

        let (tx, rx) = std::sync::mpsc::channel();
        let r2 = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(
                resolve(&AtRef::parse("@./").expect("parse"), &r2)
                    .map(|p| p.files.len())
                    .map_err(|e| format!("{e:?}")),
            );
        });
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("REFUTED: the @dir walk blocked on a FIFO in the tree");
        assert!(got.is_ok(), "walk errored: {got:?}");
    }
}
