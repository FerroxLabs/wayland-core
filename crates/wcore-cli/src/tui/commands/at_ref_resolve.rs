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
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use super::at_ref_guard::{GitIgnore, is_secret_path};
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
/// Both guards decide on the RESOLVED location, and the bytes returned come
/// from the handle whose identity was checked against it — see [`admit`].
fn resolve_file(path: &Path, root: &Path) -> Result<AtPayload, AtRefError> {
    let full = resolve_under_root(path, root);

    // The lexical floor, kept FIRST so `@.env` is refused loudly even when no
    // such file exists — a name on the denylist must not degrade to
    // "not found", which reads as "you may retry with a better spelling".
    if is_secret_path(&full) {
        return Err(AtRefError::SecretBlocked(display(path)));
    }
    if !full.is_file() {
        return Err(AtRefError::NotFound(display(path)));
    }

    let admitted = admit(&full, path)?;

    // core#339: the authoritative check is on what the path RESOLVES to.
    // `ln -s ~/.git-credentials notes.txt` clears the lexical floor above and
    // is caught here.
    if is_secret_path(&admitted.canonical) {
        return Err(AtRefError::SecretBlocked(display(path)));
    }
    // core#335: the workspace `.gitignore`'s jurisdiction follows the resolved
    // location, not the spelling. `@../<repo>/build/out.log` and a symlink into
    // the workspace both name workspace files and are judged as such; a path
    // that genuinely resolves OUTSIDE the workspace stays attachable and out of
    // the workspace gitignore's reach, which is the documented capability.
    if let Some(rel) = rel_to_root(&admitted.canonical, &canonical_root(root))
        && GitIgnore::load(root).is_ignored(&rel, false)
    {
        return Err(AtRefError::GitIgnored(display(path)));
    }

    let content = admitted.read_to_string(path)?;

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

    let ignore = GitIgnore::load(root);
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped = 0usize;
    let mut truncated = false;
    let root_canonical = canonical_root(root);
    let mut visited = HashSet::new();

    walk_dir(
        &full,
        root,
        &root_canonical,
        &ignore,
        &mut visited,
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
///
/// `root_canonical` and `visited` exist for core#339. The walk is the call site
/// that matters most there: it pulls a link in without the user ever naming it,
/// so a `notes.txt -> ~/.git-credentials` planted in a cloned repo was inlined
/// by `@./` alone. Every entry is therefore judged by what it RESOLVES to —
/// which also means a directory can be reached twice, so `visited` keeps a
/// link back into the tree from recursing until the stack runs out.
#[allow(clippy::too_many_arguments)]
fn walk_dir(
    dir: &Path,
    root: &Path,
    root_canonical: &Path,
    ignore: &GitIgnore,
    visited: &mut HashSet<PathBuf>,
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
        let is_dir = path.is_dir();
        let rel = match rel_to_root(&path, root) {
            Some(r) => r,
            None => continue,
        };
        if ignore.is_ignored(&rel, is_dir) {
            *skipped += 1;
            continue;
        }
        if is_dir {
            // An entry the walk cannot resolve is not descended into: without
            // a resolved location there is nothing to judge scope by.
            let Ok(canonical) = fs::canonicalize(&path) else {
                *skipped += 1;
                continue;
            };
            // A link OUT of the workspace is not part of the directory the
            // user asked for, and reaching through one is how `@./` walks into
            // `$HOME`.
            if !canonical.starts_with(root_canonical) {
                *skipped += 1;
                continue;
            }
            // core#322 c4: a VCS control directory or content store is never
            // useful context, can be enormous, and reconstructs committed
            // secrets through its own porcelain. This was a literal `.git`
            // NAME test, which missed `.hg`/`.svn`/`.bzr` outright and missed
            // a `.git` reached under any other name. The shape test is the one
            // `wcore-tools`' deny walk uses — one list, one owner — and it is
            // asked about the RESOLVED path, so the entry's own name is
            // irrelevant.
            if wcore_tools::workspace_policy::is_vcs_store_or_control_dir(&canonical) {
                continue;
            }
            // core#339 c6: `.gitignore` is judged on where the entry resolves,
            // for the same reason the secret guard is.
            if rel_to_root(&canonical, root_canonical)
                .is_some_and(|rel| ignore.is_ignored(&rel, true))
            {
                *skipped += 1;
                continue;
            }
            // Reached twice (a link back into the tree) is walked once.
            if !visited.insert(canonical) {
                continue;
            }
            walk_dir(
                &path,
                root,
                root_canonical,
                ignore,
                visited,
                out,
                skipped,
                truncated,
            )?;
        } else {
            // Resolve once; guard the resolved name; read the same handle.
            let Ok(admitted) = admit(&path, &path) else {
                *skipped += 1;
                continue;
            };
            if !admitted.canonical.starts_with(root_canonical)
                || is_secret_path(&path)
                || is_secret_path(&admitted.canonical)
            {
                *skipped += 1;
                continue;
            }
            // core#339 c6: the `rel` above is the LEXICAL entry, so an in-root
            // link named `notes.txt` at an in-root `deploy.log` was judged as
            // `notes.txt` and no `*.log` rule ever saw it. Judge the rule on
            // what the entry RESOLVES to, as `resolve_file` already does
            // (core#335).
            if rel_to_root(&admitted.canonical, root_canonical)
                .is_some_and(|rel| ignore.is_ignored(&rel, false))
            {
                *skipped += 1;
                continue;
            }
            // Read text files only; a binary file is skipped silently
            // rather than corrupting the payload with lossy bytes.
            match admitted.read_to_string(&path) {
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

/// One file, opened ONCE, together with the symlink-free location the guards
/// must decide on.
///
/// core#339: the guard matched the LEXICAL path while the read followed the
/// link, so `ln -s ~/.git-credentials notes.txt` made `@notes.txt` inline a
/// credential store. Deciding on the resolved path is only half the answer —
/// canonicalizing and then RE-OPENING BY PATH reintroduces the race the link
/// was planted for, because the link can be repointed in between. So the handle
/// below is the one the caller reads from, and nothing re-opens the path after
/// the guards have run.
struct Admitted {
    /// The open handle whose identity was matched against `canonical`. The
    /// bytes read come from HERE, never from a second open.
    handle: same_file::Handle,
    /// Where the reference actually resolves — symlinks and `..` removed.
    /// Both guards run against this.
    canonical: PathBuf,
}

impl Admitted {
    /// Read the admitted file. Consumes the handle, so no path is re-opened
    /// between the guard and the bytes. `spelled` names the file in errors —
    /// the user's own spelling, not the resolved one.
    fn read_to_string(mut self, spelled: &Path) -> Result<String, AtRefError> {
        let mut content = String::new();
        self.handle
            .as_file_mut()
            .read_to_string(&mut content)
            .map(|_| content)
            .map_err(|e| AtRefError::Io {
                path: display(spelled),
                message: e.to_string(),
            })
    }
}

/// Read one file for a `@`-surface read site under the secret guard: resolve
/// once, decide on the resolved name, and return the bytes of the very handle
/// that was decided on.
///
/// core#339 c3: three of the four read sites on this surface got the
/// resolved-path guard and the fourth — `at_ref_send::read_def_snippet`, the
/// `@symbol` preview — was missed, still calling `fs::read_to_string` on a
/// repomap-supplied path. The guard lives HERE rather than being copied there,
/// because two copies of a guard that must agree are how this surface grew four
/// read sites with three answers in the first place.
pub(super) fn read_guarded(path: &Path) -> Result<String, AtRefError> {
    // The lexical floor first, so a denylisted NAME is refused loudly even when
    // the path does not resolve — see `resolve_file` for why that order matters.
    if is_secret_path(path) {
        return Err(AtRefError::SecretBlocked(display(path)));
    }
    let admitted = admit(path, path)?;
    if is_secret_path(&admitted.canonical) {
        return Err(AtRefError::SecretBlocked(display(path)));
    }
    admitted.read_to_string(path)
}

/// Open `full` once and resolve what it names, refusing the reference when the
/// two answers describe different files.
///
/// `same_file::Handle` is device+inode on Unix and volume-serial + file-index
/// on Windows, so this is one portable identity question rather than a
/// `cfg`-split of two. An attacker who repoints the link between the open and
/// the `canonicalize` makes the identities disagree, and the reference is
/// refused instead of being guarded under one name and read from another.
fn admit(full: &Path, spelled: &Path) -> Result<Admitted, AtRefError> {
    let io = |e: std::io::Error| AtRefError::Io {
        path: display(spelled),
        message: e.to_string(),
    };
    let handle = same_file::Handle::from_path(full).map_err(io)?;
    let canonical = fs::canonicalize(full).map_err(io)?;
    let named = same_file::Handle::from_path(&canonical).map_err(io)?;
    if handle != named {
        return Err(AtRefError::Io {
            path: display(spelled),
            message: "the path changed while it was being resolved".to_string(),
        });
    }
    Ok(Admitted { handle, canonical })
}

/// The workspace root with symlinks resolved, so scope decisions compare like
/// with like (core#335 / core#339).
///
/// Falls back to the root as given when it cannot be canonicalized: a root that
/// does not resolve cannot contain anything, and every path is then simply
/// treated as outside it — the conservative direction for a guard whose "inside"
/// answer only ever ADDS a check.
fn canonical_root(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

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

    // ── core#339 / core#335: the guard must decide by what a path RESOLVES
    //    to, not by the shape of the string the user typed ────────────────

    /// A credential store with a recognisable body, so a test can assert the
    /// bytes never reached a payload without printing them.
    const CREDENTIAL_BODY: &str = "https://user:s3cr3t-token@git.example.com\n";

    /// core#339 — `@notes.txt` where `notes.txt` is a symlink to
    /// `~/.git-credentials`. The guard matched the LEXICAL name, found
    /// nothing on either denylist, and `fs::read_to_string` then followed the
    /// link and inlined the credential store into the outgoing prompt.
    #[cfg(unix)]
    #[test]
    fn at_file_refuses_a_symlink_whose_target_is_a_credential_store() {
        let outside = TempDir::new().expect("tempdir");
        let secret = outside.path().join(".git-credentials");
        fs::write(&secret, CREDENTIAL_BODY).expect("write secret");

        let tmp = TempDir::new().expect("tempdir");
        std::os::unix::fs::symlink(&secret, tmp.path().join("notes.txt")).expect("symlink");

        let at = AtRef::parse("@notes.txt").expect("parse");
        match resolve(&at, tmp.path()) {
            Err(AtRefError::SecretBlocked(_)) => {}
            Err(other) => panic!("expected SecretBlocked, got {other:?}"),
            Ok(payload) => panic!(
                "the credential store was inlined into the prompt: {} bytes, \
                 credential present = {}",
                payload.bytes(),
                payload.files[0].content.contains("s3cr3t-token")
            ),
        }
    }

    /// core#339, the walk arm. The `@dir` walk pulls such a link in without
    /// the user ever naming it — the more dangerous of the three call sites.
    #[cfg(unix)]
    #[test]
    fn at_dir_never_walks_a_symlink_into_a_credential_store() {
        let outside = TempDir::new().expect("tempdir");
        let secret = outside.path().join(".git-credentials");
        fs::write(&secret, CREDENTIAL_BODY).expect("write secret");

        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("ok.txt"), "safe").expect("write ok");
        std::os::unix::fs::symlink(&secret, root.join("notes.txt")).expect("symlink");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");
        let leaked = payload
            .files
            .iter()
            .any(|f| f.content.contains("s3cr3t-token"));
        assert!(
            !leaked,
            "the @dir walk inlined a credential store reached through a symlink"
        );
        assert!(
            payload.files.iter().any(|f| f.content == "safe"),
            "the ordinary file in the same directory must still be attached"
        );
    }

    /// core#339 negative control — a symlink is NOT a secret. A repo that
    /// symlinks an ordinary file must keep working; a guard that refuses
    /// every link is not a fix. Passes on BOTH arms.
    #[cfg(unix)]
    #[test]
    fn at_file_still_attaches_a_symlink_to_an_ordinary_file() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("real.md"), "real content\n").expect("write");
        std::os::unix::fs::symlink(root.join("real.md"), root.join("link.md")).expect("symlink");

        let at = AtRef::parse("@link.md").expect("parse");
        let payload = resolve(&at, root).expect("an ordinary symlink is not a secret");
        assert_eq!(payload.files[0].content, "real content\n");
    }

    /// core#335 — the gitignore check is skipped whenever `strip_prefix`
    /// cannot see that the path is inside the root. `@../<repo>/build/out.log`
    /// names a file that RESOLVES inside the workspace and is ignored by the
    /// workspace `.gitignore`, but `rel_to_root` returns `None` for the
    /// `..`-bearing spelling and the guard is never consulted.
    #[test]
    fn a_dotdot_spelling_does_not_escape_the_workspace_gitignore() {
        let parent = TempDir::new().expect("tempdir");
        let root = parent.path().join("repo");
        fs::create_dir_all(root.join("build")).expect("mkdir");
        fs::write(root.join(".gitignore"), "*.log\n").expect("write gitignore");
        fs::write(root.join("build/out.log"), "build log\n").expect("write log");

        // The plain spelling IS refused — proof the fixture is really ignored,
        // and it passes on both arms.
        let plain = resolve(&AtRef::parse("@build/out.log").expect("parse"), &root);
        assert!(
            matches!(plain, Err(AtRefError::GitIgnored(_))),
            "fixture check: the relative spelling must be refused, got {plain:?}"
        );

        // The same file, spelled to defeat the lexical prefix test.
        let at = AtRef::parse("@../repo/build/out.log").expect("parse");
        let got = resolve(&at, &root);
        assert!(
            matches!(got, Err(AtRefError::GitIgnored(_))),
            "a git-ignored file must stay refused however it is spelled, got {got:?}"
        );
    }

    /// core#335 negative control — attaching a file from OUTSIDE the
    /// workspace by absolute path is a documented capability, not a bypass.
    /// It must keep working, and the workspace `.gitignore` must not reach
    /// out of the workspace to veto it. Passes on BOTH arms.
    #[test]
    fn an_absolute_path_outside_the_workspace_still_attaches() {
        let outside = TempDir::new().expect("tempdir");
        let note = outside.path().join("out.log");
        fs::write(&note, "outside content\n").expect("write");

        let tmp = TempDir::new().expect("tempdir");
        // The workspace ignores `*.log`; the file is not in the workspace.
        fs::write(tmp.path().join(".gitignore"), "*.log\n").expect("write gitignore");

        let payload = resolve(&AtRef::File(note.clone()), tmp.path())
            .expect("an explicit absolute attach is a capability, not a bypass");
        assert_eq!(payload.files[0].content, "outside content\n");
    }
    // ── core#339 c6 / core#322 c4: the @dir walk's remaining lexical
    //    judgements — the gitignore match and the VCS-store skip ────────────

    /// A committed object body with a recognisable marker, so a test can assert
    /// the bytes never reached a payload without printing them.
    const COMMITTED_OBJECT: &str = "COMMITTED-OBJECT s3cr3t-blob\n";

    /// core#339 c6 — the walk matched `.gitignore` on the LEXICAL entry, so an
    /// in-root link to an in-root ignored file was laundered past the rule by
    /// its own spelling. `resolve_file` already judges the resolved path
    /// (core#335); the walk did not.
    #[cfg(unix)]
    #[test]
    fn at_dir_judges_gitignore_on_the_resolved_path() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "*.log\n").expect("write gitignore");
        fs::write(root.join("deploy.log"), "IGNORED-BUILD-OUTPUT\n").expect("write log");
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        // The same ignored file under a name the `*.log` rule does not match.
        std::os::unix::fs::symlink(root.join("deploy.log"), root.join("notes.txt"))
            .expect("symlink");
        // Wrong-refusal control: a link to a file the rule does NOT cover stays
        // attached, so the fix cannot be "skip every link". Passes on BOTH arms.
        std::os::unix::fs::symlink(root.join("ok.txt"), root.join("alias.txt")).expect("symlink");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let names: Vec<String> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("IGNORED-BUILD-OUTPUT")),
            "a git-ignored file was attached through a link named around the rule: {names:?}"
        );
        assert_eq!(
            payload
                .files
                .iter()
                .filter(|f| f.content == "safe\n")
                .count(),
            2,
            "the ordinary file AND its link to a non-ignored file must both stay attached: {names:?}"
        );
    }

    /// core#322 c4 — the walk skipped a VCS object store only when the entry
    /// was LITERALLY named `.git`, so the same store reached under any other
    /// name (an in-root link, a vendored checkout's `.hg/store` aliased) was
    /// walked and every committed object under it inlined. This is the class
    /// #322 closed on the `wcore-tools` deny walk; the composer surface had no
    /// equivalent.
    #[cfg(unix)]
    #[test]
    fn at_dir_never_walks_a_vcs_store_reached_under_another_name() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join(".git/objects/aa")).expect("mkdir git store");
        fs::write(root.join(".git/objects/aa/deadbeef"), COMMITTED_OBJECT).expect("write object");
        fs::create_dir_all(root.join(".hg/store/data")).expect("mkdir hg store");
        fs::write(root.join(".hg/store/data/notes.i"), COMMITTED_OBJECT).expect("write revlog");

        // The same stores, reached under names the literal `.git` test misses.
        std::os::unix::fs::symlink(root.join(".git"), root.join("mirror")).expect("symlink");
        std::os::unix::fs::symlink(root.join(".hg/store"), root.join("vendor")).expect("symlink");

        // Wrong-refusal control: an ordinary directory whose name merely
        // resembles one, and an ordinary file, must still be attached.
        fs::create_dir_all(root.join("gitignore-docs")).expect("mkdir docs");
        fs::write(root.join("gitignore-docs/notes.md"), "ordinary\n").expect("write notes");
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let leaked: Vec<String> = payload
            .files
            .iter()
            .filter(|f| f.content.contains("COMMITTED-OBJECT"))
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            leaked.is_empty(),
            "a VCS content store was walked under another name: {leaked:?}"
        );
        assert!(
            payload.files.iter().any(|f| f.content == "ordinary\n"),
            "control: an ordinary directory must still be walked"
        );
        assert!(
            payload.files.iter().any(|f| f.content == "safe\n"),
            "control: an ordinary file must still be attached"
        );
    }

    /// core#335 — the ABSOLUTE escaping spelling, the one the issue is named
    /// for, which nothing pinned. The `..`-relative arm above survives with
    /// the root left uncanonicalized, so `canonical_root` — the whole
    /// load-bearing half of the absolute case — could be deleted with every
    /// shipped `at_ref_resolve` test still green.
    ///
    /// An absolute spelling only escapes when the ROOT's own spelling is not
    /// canonical, which is ordinary rather than exotic: a symlinked checkout,
    /// and every workspace under macOS's `/tmp` (`-> /private/tmp`) or `/var`
    /// (`-> /private/var`), where the absolute path a user copies out of their
    /// shell is spelled in the canonical form the root is not.
    #[test]
    fn an_absolute_spelling_does_not_escape_the_workspace_gitignore() {
        // Portable arm: a root spelled out through a child and back. It names
        // the same directory, and `strip_prefix` cannot see that it does.
        let tmp = TempDir::new().expect("tempdir");
        fs::create_dir_all(tmp.path().join("build")).expect("mkdir build");
        fs::create_dir_all(tmp.path().join("sub")).expect("mkdir sub");
        fs::write(tmp.path().join(".gitignore"), "*.log\n").expect("write gitignore");
        fs::write(tmp.path().join("build/out.log"), "build log\n").expect("write log");
        let root = tmp.path().join("sub").join("..");

        // FIXTURE CONTROL, spelled against the CANONICAL root so it is
        // insensitive to the property under test: the file really is ignored.
        // Passes on BOTH arms.
        let plain = resolve(&AtRef::parse("@build/out.log").expect("parse"), tmp.path());
        assert!(
            matches!(plain, Err(AtRefError::GitIgnored(_))),
            "fixture check: the relative spelling must be refused, got {plain:?}"
        );

        // The same file, named by an absolute path whose place inside the root
        // no lexical prefix test can see.
        let absolute = tmp.path().join("build").join("out.log");
        let got = resolve(&AtRef::File(absolute), &root);
        assert!(
            matches!(got, Err(AtRefError::GitIgnored(_))),
            "an absolute spelling of an in-workspace ignored file must stay refused, got {got:?}"
        );

        // Unix arm: the same escape in its realistic shape, a symlinked root.
        #[cfg(unix)]
        {
            let outer = TempDir::new().expect("tempdir");
            let real = outer.path().join("real");
            fs::create_dir_all(real.join("build")).expect("mkdir");
            fs::write(real.join(".gitignore"), "*.log\n").expect("write gitignore");
            fs::write(real.join("build/out.log"), "build log\n").expect("write log");
            let linked_root = outer.path().join("workspace");
            std::os::unix::fs::symlink(&real, &linked_root).expect("symlink root");

            let got = resolve(
                &AtRef::File(real.join("build").join("out.log")),
                &linked_root,
            );
            assert!(
                matches!(got, Err(AtRefError::GitIgnored(_))),
                "a symlinked workspace root must still apply its own gitignore to an \
                 absolute spelling, got {got:?}"
            );

            // WRONG-REFUSAL CONTROL, on the same fixture: a file that really is
            // outside the workspace stays attachable — the behaviour core#335
            // c1 decided to keep — so this cannot be satisfied by refusing
            // every absolute path. Passes on BOTH arms.
            let outside = outer.path().join("elsewhere.log");
            fs::write(&outside, "outside content\n").expect("write outside");
            let payload = resolve(&AtRef::File(outside), &linked_root)
                .expect("an explicit absolute attach from outside is a capability");
            assert_eq!(payload.files[0].content, "outside content\n");
        }
    }

    /// core#339 — the walk's WORKSPACE CONFINEMENT, which replaced the
    /// criterion's `symlink_metadata` mechanism (a blanket symlink refusal the
    /// issue explicitly forbids) and was then graded by nothing: both scope
    /// checks could be replaced by `if false` with all 65 `at_ref` tests still
    /// green. The suite's only escape fixture is NAMED `.git-credentials`, so
    /// `is_secret_path` alone satisfies it and the confinement is never asked.
    ///
    /// Nothing outside this fixture is on any denylist, so the scope check is
    /// the only thing keeping it out of the payload.
    #[cfg(unix)]
    #[test]
    fn at_dir_never_reaches_outside_the_workspace_through_a_symlink() {
        const OUTSIDE: &str = "PRIVATE-OUTSIDE-PAYLOAD";

        let elsewhere = TempDir::new().expect("tempdir");
        fs::write(
            elsewhere.path().join("taxes.txt"),
            format!("{OUTSIDE} taxes\n"),
        )
        .expect("write outside file");
        let archive = elsewhere.path().join("archive");
        fs::create_dir_all(&archive).expect("mkdir archive");
        fs::write(archive.join("notes.md"), format!("{OUTSIDE} notes\n"))
            .expect("write outside notes");

        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        // A link OUT to an ordinary-looking file, and a link OUT to a whole
        // directory. Neither name appears on any denylist.
        std::os::unix::fs::symlink(elsewhere.path().join("taxes.txt"), root.join("notes.txt"))
            .expect("symlink file");
        std::os::unix::fs::symlink(&archive, root.join("escape")).expect("symlink dir");
        // The observable for the DIRECTORY half. A file outside the root is
        // refused by the FILE scope check even if the walk descends, so on its
        // own an outside tree cannot tell the two checks apart. This link
        // points back at an in-root file, so it clears the file check and is
        // attached if and only if the walk descended through `escape`.
        std::os::unix::fs::symlink(root.join("ok.txt"), archive.join("back.txt"))
            .expect("symlink back");
        // WRONG-REFUSAL CONTROL: an IN-root link must still be attached, so
        // the fix cannot be "skip every symlink" — the shape core#339
        // explicitly rejects. Passes on BOTH arms.
        std::os::unix::fs::symlink(root.join("ok.txt"), root.join("alias.txt")).expect("symlink");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let names: Vec<String> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            !payload.files.iter().any(|f| f.content.contains(OUTSIDE)),
            "a file outside the workspace was inlined through an in-root symlink: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with("escape/")),
            "the walk descended through a symlink out of the workspace: {names:?}"
        );
        assert_eq!(
            payload
                .files
                .iter()
                .filter(|f| f.content == "safe\n")
                .count(),
            2,
            "control: the in-root file and its in-root link must both stay attached: {names:?}"
        );
    }

    /// core#339 — the walk's SECRET DENYLIST, asked about what the entry
    /// RESOLVES to. `at_dir_never_walks_a_symlink_into_a_credential_store`
    /// looks like it pins this and does not: its `.git-credentials` sits
    /// OUTSIDE the root, so the scope check refuses it before the denylist is
    /// ever consulted, and `is_secret_path(&admitted.canonical)` could be
    /// deleted with all 67 `at_ref` tests green.
    ///
    /// The reachable shape is an IN-root link at an IN-root secret, where the
    /// scope check has nothing to say and only the resolved-name test stands
    /// between `@./` and the credential body.
    #[cfg(unix)]
    #[test]
    fn at_dir_judges_the_secret_denylist_on_the_resolved_path() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".env"), CREDENTIAL_BODY).expect("write env");
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        // In root, at an in-root secret, under a name no denylist carries.
        std::os::unix::fs::symlink(root.join(".env"), root.join("notes.txt")).expect("symlink");
        // WRONG-REFUSAL CONTROL: an in-root link at an ordinary in-root file
        // stays attached. Passes on BOTH arms.
        std::os::unix::fs::symlink(root.join("ok.txt"), root.join("alias.txt")).expect("symlink");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let names: Vec<String> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("s3cr3t-token")),
            "the @dir walk inlined a secret reached through a link named around \
             the denylist: {names:?}"
        );
        assert_eq!(
            payload
                .files
                .iter()
                .filter(|f| f.content == "safe\n")
                .count(),
            2,
            "control: the ordinary file and its link must both stay attached: {names:?}"
        );
    }

    /// core#339 c6, the DIRECTORY half. `at_dir_judges_gitignore_on_the_resolved_path`
    /// covers the file branch only, so the dir branch's resolved-path rule
    /// check could be deleted with every `at_ref` test green — and a
    /// directory-only rule (`build/`) is skipped outright by the file check
    /// (`is_ignored` returns early when `dir_only && !is_dir`), so nothing
    /// downstream catches the entries either.
    #[cfg(unix)]
    #[test]
    fn at_dir_judges_a_directory_gitignore_rule_on_the_resolved_path() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "build/\n").expect("write gitignore");
        fs::create_dir_all(root.join("build")).expect("mkdir build");
        fs::write(root.join("build/out.txt"), "IGNORED-TREE\n").expect("write ignored");
        // The same ignored directory under a name the rule does not match.
        std::os::unix::fs::symlink(root.join("build"), root.join("docs")).expect("symlink");
        // WRONG-REFUSAL CONTROL: an ordinary directory reached through a link
        // is still walked, so the fix cannot be "skip every dir symlink".
        // `visited` walks an aliased directory ONCE, hence `any` and not a
        // count of two. Passes on BOTH arms.
        fs::create_dir_all(root.join("src")).expect("mkdir src");
        fs::write(root.join("src/main.rs"), "safe\n").expect("write src");
        std::os::unix::fs::symlink(root.join("src"), root.join("lib")).expect("symlink src");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let names: Vec<String> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("IGNORED-TREE")),
            "a git-ignored directory was walked through a link named around the rule: {names:?}"
        );
        assert!(
            payload.files.iter().any(|f| f.content == "safe\n"),
            "control: an ordinary directory reached through a link must still be walked: {names:?}"
        );
    }

    /// The LEXICAL floor — the three name-based checks that run before the
    /// resolved-path ones. A mutation sweep found all three ungraded: deleting
    /// `is_secret_path(&full)` (`resolve_file`), the walk's
    /// `ignore.is_ignored(&rel, is_dir)`, or the walk's `is_secret_path(&path)`
    /// each left all 69 `at_ref` tests green, because for an ordinary entry the
    /// lexical name and the resolved name are the same and the resolved checks
    /// answer identically.
    ///
    /// They are not redundant. Each earns its keep exactly where the two
    /// answers DIVERGE — a denylisted or ignored NAME whose target is
    /// innocuous — and on the one case with no resolved path at all: a
    /// denylisted name that does not exist must be refused LOUDLY, because
    /// "not found" reads as "retry with a better spelling".
    #[cfg(unix)]
    #[test]
    fn the_lexical_floor_refuses_a_denylisted_or_ignored_name_whatever_it_resolves_to() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "*.log\n").expect("write gitignore");
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        // A denylisted NAME and an ignored NAME, both pointing at an innocuous
        // file — so only the lexical answer refuses them.
        std::os::unix::fs::symlink(root.join("ok.txt"), root.join(".env")).expect("symlink env");
        std::os::unix::fs::symlink(root.join("ok.txt"), root.join("deploy.log"))
            .expect("symlink log");

        // `@.env` with nothing behind it at all is still a loud refusal, not a
        // NotFound the user would read as "try another spelling".
        let missing = resolve(&AtRef::parse("@nope/.env").expect("parse"), root);
        assert!(
            matches!(missing, Err(AtRefError::SecretBlocked(_))),
            "a denylisted name that does not exist must be refused loudly, got {missing:?}"
        );
        // `read_guarded` — the shared helper behind the `@symbol` preview —
        // states the same contract in its own comment and had the same gap.
        let previewed = read_guarded(&root.join("nope").join(".env"));
        assert!(
            matches!(previewed, Err(AtRefError::SecretBlocked(_))),
            "the @symbol preview must refuse a denylisted name loudly even when it \
             does not resolve, got {previewed:?}"
        );

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let names: Vec<String> = payload
            .files
            .iter()
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            !names.iter().any(|n| n == ".env"),
            "a denylisted NAME must not be attached however innocuous its target: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "deploy.log"),
            "an ignored NAME must not be attached however innocuous its target: {names:?}"
        );
        // WRONG-REFUSAL CONTROL: the ordinary file is still attached, so the
        // floor cannot be satisfied by refusing everything. Passes on BOTH arms.
        assert!(
            names.iter().any(|n| n == "ok.txt"),
            "control: the ordinary file must still be attached: {names:?}"
        );
    }
}
