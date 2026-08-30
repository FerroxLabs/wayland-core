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
    let base_canonical = canonical_root(&full);
    let scope = WalkScope {
        root,
        root_canonical: &root_canonical,
        base: &full,
        base_canonical: &base_canonical,
        spelled: path,
    };
    let mut visited = HashSet::new();

    walk_dir(
        &full,
        &scope,
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

/// The two directories a walked entry is judged against.
///
/// D3: they were one. `walk_dir` named every entry relative to the WORKSPACE
/// root and dropped — silently, without even counting a skip — any entry the
/// root could not name. Every `@dir` spelling that escapes the root lexically
/// (`@../repo/`, `@/abs/dir/`) therefore resolved to an empty payload behind a
/// successful-looking chip: the user attached a directory and the model got
/// nothing. The workspace root is the `.gitignore`'s jurisdiction; the
/// directory the reference NAMES is what the walk may not leave.
struct WalkScope<'a> {
    /// The workspace root, as the caller spelled it.
    root: &'a Path,
    /// The workspace root with symlinks resolved.
    root_canonical: &'a Path,
    /// The directory the reference names, and its canonical form. For an
    /// in-root `@dir` this sits under the workspace root, so core#339's
    /// confinement is exactly what it was.
    base: &'a Path,
    base_canonical: &'a Path,
    /// The user's own spelling of that directory, echoed back in the names of
    /// entries the workspace root cannot name — the same answer `resolve_file`
    /// gives for an escaping `@file`.
    spelled: &'a Path,
}

impl WalkScope<'_> {
    /// `path` relative to the workspace root, or `None` when the workspace
    /// cannot name it. `None` means "outside the `.gitignore`'s jurisdiction",
    /// never "invisible".
    fn in_root(&self, path: &Path) -> Option<String> {
        rel_to_root(path, self.root)
    }

    /// The same question asked of a RESOLVED location, for the rules that are
    /// judged on where an entry actually points (core#335 / core#339 c6).
    fn canonical_in_root(&self, canonical: &Path) -> Option<String> {
        rel_to_root(canonical, self.root_canonical)
    }

    /// True when a resolved location is inside the workspace or inside the
    /// directory the reference names. Everything else is a link OUT of what
    /// the user asked for.
    fn contains(&self, canonical: &Path) -> bool {
        canonical.starts_with(self.root_canonical) || canonical.starts_with(self.base_canonical)
    }

    /// The name the payload carries for `path`.
    fn name(&self, path: &Path) -> Option<PathBuf> {
        match self.in_root(path) {
            Some(rel) => Some(PathBuf::from(rel)),
            None => Some(self.spelled.join(rel_to_root(path, self.base)?)),
        }
    }
}

/// Depth-first directory walk for `@dir`, applying both guardrails.
///
/// `root_canonical` and `visited` exist for core#339. The walk is the call site
/// that matters most there: it pulls a link in without the user ever naming it,
/// so a `notes.txt -> ~/.git-credentials` planted in a cloned repo was inlined
/// by `@./` alone. Every entry is therefore judged by what it RESOLVES to —
/// which also means a directory can be reached twice, so `visited` keeps a
/// link back into the tree from recursing until the stack runs out.
fn walk_dir(
    dir: &Path,
    scope: &WalkScope<'_>,
    ignore: &GitIgnore,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<ResolvedFile>,
    skipped: &mut usize,
    truncated: &mut bool,
) -> Result<(), AtRefError> {
    // NOT A DROP: truncation is already reported. `*truncated` is the
    // payload's own signal and `resolve` turns it into a warning, so returning
    // here withholds nothing the caller is not being told about.
    if *truncated {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| AtRefError::Io {
        path: display(dir),
        message: e.to_string(),
    })?;

    // Sort entries for a deterministic walk — the payload (and its tests)
    // must not depend on filesystem iteration order.
    //
    // core#377 c2. This was `entries.flatten().map(|e| e.path()).collect()`.
    // `flatten()` discards every `Err` the `ReadDir` iterator yields, so an
    // entry the OS refused to describe never reached the loop below, was never
    // counted, and produced no `SkippedFiles` warning — c2's sentence is
    // "`AtWarning::SkippedFiles` is emitted whenever any entry is dropped", and
    // this dropped entries. `every_silent_exit_in_walk_dir_is_counted_or_
    // justified` could not see it either: an adapter is not an EXIT, so the
    // drop happened before the region that gate grades. The second gate,
    // `the_readdir_iterator_is_consumed_only_by_a_bare_for_loop`, is what
    // makes the whole class unwritable.
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            // An entry the OS would not describe is an entry this walk drops,
            // and a drop is exactly what the counter is for.
            *skipped += 1;
            continue;
        };
        paths.push(entry.path());
    }
    paths.sort();

    for path in paths {
        // NOT A DROP: the entries after the cap are withheld, but `*truncated`
        // is set on the line above and carries that fact to the user. This is
        // the one exit that ends the walk while saying so by a different means
        // than the skipped counter.
        if out.len() >= DIR_MAX_FILES {
            *truncated = true;
            return Ok(());
        }
        let is_dir = path.is_dir();
        // D3: an entry the workspace root cannot NAME is out of the workspace
        // `.gitignore`'s jurisdiction — it is not invisible. Dropping it here
        // is what made every escaping `@dir` spelling resolve to an empty
        // payload, and without `*skipped += 1` there was not even a warning.
        if let Some(rel) = scope.in_root(&path)
            && ignore.is_ignored(&rel, is_dir)
        {
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
            if !scope.contains(&canonical) {
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
            //
            // The predicate tests the path AND its ancestors, because pruning
            // alone does not cover this walk: a symlink aimed BELOW a store
            // root is met at the top of the tree and never descended TO, so a
            // self-only test admitted `.git/objects/aa` and inlined the
            // objects under it.
            //
            // FerroxLabs/wayland-core#377 c2 — this IS a drop and it is now
            // counted. A pruned store is content the user asked for and did
            // not get; one entry, not one file, because the walk deliberately
            // never learns how many files are under it. Silence here was the
            // last uncounted drop in this function.
            if wcore_tools::workspace_policy::is_within_vcs_store_or_control_dir(&canonical) {
                *skipped += 1;
                continue;
            }
            // core#339 c6: `.gitignore` is judged on where the entry resolves,
            // for the same reason the secret guard is.
            if scope
                .canonical_in_root(&canonical)
                .is_some_and(|rel| ignore.is_ignored(&rel, true))
            {
                *skipped += 1;
                continue;
            }
            // Reached twice (a link back into the tree) is walked once.
            //
            // NOT A DROP: the entry is not missing from the payload, it is
            // already IN it — this arm runs only after the first visit walked
            // the same canonical directory. Counting it would report a skip
            // for content the user did receive, which is a different lie from
            // the silence FerroxLabs/wayland-core#377 was filed about.
            // Graded by `a_directory_reached_twice_is_walked_once_and_not_counted_as_skipped`.
            if !visited.insert(canonical) {
                continue;
            }
            walk_dir(&path, scope, ignore, visited, out, skipped, truncated)?;
        } else {
            // D7: `admit` OPENS the entry, and opening a named pipe blocks
            // until a writer appears — `@./` in any tree containing a FIFO
            // (build systems, editors and language servers all leave them)
            // wedged the turn forever, inside a blocking syscall on the turn
            // task where cancellation cannot reach it. Only a regular file is
            // readable; `resolve_file` has always said so, the walk never did.
            // `metadata` follows the link and only stats, so it cannot block.
            let Ok(meta) = fs::metadata(&path) else {
                *skipped += 1;
                continue;
            };
            if !meta.is_file() {
                *skipped += 1;
                continue;
            }
            // Resolve once; guard the resolved name; read the same handle.
            let Ok(admitted) = admit(&path, &path) else {
                *skipped += 1;
                continue;
            };
            if !scope.contains(&admitted.canonical)
                || is_secret_path(&path)
                || is_secret_path(&admitted.canonical)
                // core#322 c4: the same reach on the FILE arm. `is_secret_path`
                // matches secret NAMES and an object file is named after its
                // hash, so a link straight at `.git/objects/aa/deadbeef` was
                // read and inlined without any store predicate being consulted.
                || wcore_tools::workspace_policy::is_within_vcs_store_or_control_dir(
                    &admitted.canonical,
                )
            {
                *skipped += 1;
                continue;
            }
            // core#339 c6: the gitignore test at the top of the loop is on the
            // LEXICAL entry, so an in-root link named `notes.txt` at an in-root
            // `deploy.log` was judged as `notes.txt` and no `*.log` rule ever saw it. Judge the rule on
            // what the entry RESOLVES to, as `resolve_file` already does
            // (core#335).
            if scope
                .canonical_in_root(&admitted.canonical)
                .is_some_and(|rel| ignore.is_ignored(&rel, false))
            {
                *skipped += 1;
                continue;
            }
            let Some(name) = scope.name(&path) else {
                *skipped += 1;
                continue;
            };
            // Read text files only; a binary file is skipped silently
            // rather than corrupting the payload with lossy bytes.
            match admitted.read_to_string(&path) {
                Ok(content) => out.push(ResolvedFile {
                    path: name,
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
    // D7, the walk's sibling read site: `admit` opens the path, and opening a
    // named pipe blocks until a writer appears. Only a regular file is a
    // readable `@`-reference target.
    if !path.is_file() {
        return Err(AtRefError::NotFound(display(path)));
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

    /// core#335 c3, the `..`-relative arm of the DECIDED BEHAVIOUR (Q1,
    /// option A: escaping attachments keep working, pinned by a test). The
    /// absolute arm is `an_absolute_path_outside_the_workspace_still_attaches`
    /// above. Until this test the `..` arm was pinned by NOTHING: every `..`
    /// spelling in the suite re-enters the workspace, so a regression that
    /// refused only the escaping `..` spelling — #335's spelling-sensitivity,
    /// in the opposite direction — would have shipped with a green suite.
    #[test]
    fn a_dotdot_spelling_that_escapes_the_workspace_still_attaches() {
        let parent = TempDir::new().expect("tempdir");
        let root = parent.path().join("repo");
        fs::create_dir_all(&root).expect("mkdir root");
        fs::create_dir_all(parent.path().join("outside")).expect("mkdir outside");
        // The workspace ignores `*.log`, and the escaping target shares that
        // extension — so the only thing that can admit it is being OUTSIDE.
        fs::write(root.join(".gitignore"), "*.log\n").expect("write gitignore");
        fs::write(parent.path().join("outside/notes.log"), "outside content\n")
            .expect("write outside");
        fs::write(root.join("inside.log"), "inside content\n").expect("write inside");

        // In-fixture control: the same extension INSIDE the workspace is
        // genuinely refused, so the attach below cannot pass on a toothless
        // gitignore. Passes on BOTH arms.
        let inside = resolve(&AtRef::parse("@inside.log").expect("parse"), &root);
        assert!(
            matches!(inside, Err(AtRefError::GitIgnored(_))),
            "fixture check: the workspace gitignore must have teeth, got {inside:?}"
        );

        // The escaping `..` spelling lands outside the workspace, so the
        // workspace gitignore has no jurisdiction over it and the attach
        // stands — the decided behaviour, on the spelling c1 promised to pin.
        let at = AtRef::parse("@../outside/notes.log").expect("parse");
        let payload = resolve(&at, &root).expect(
            "a `..` spelling that lands outside the workspace is a capability, not a bypass",
        );
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

    /// core#322 c4 — THE TIEBREAK. The lane graded c4 met on a parity claim: the
    /// walk asks [`wcore_tools::workspace_policy::is_within_vcs_store_or_control_dir`],
    /// which tests the path ITSELF, and the deny walk asks `inside_vcs_store`,
    /// which tests the path and every ANCESTOR. The lane's defence was that a
    /// walk PRUNES at the control directory and therefore can never stand
    /// inside a store, making the two equivalent in effect.
    ///
    /// Pruning only governs paths the walk DESCENDS to. A symlink is an entry
    /// the walk meets at the top of the tree, and one aimed BELOW a store's own
    /// root — `.git/objects/aa`, not `.git` and not `.git/objects` — resolves to
    /// a path that is neither a store shape (`objects/aa` is not a
    /// (control, store) pair) nor a control-directory leaf. The self-test says
    /// walk it; the ancestor test says deny it; the difference is a committed
    /// object in the payload.
    #[cfg(unix)]
    #[test]
    fn at_dir_prunes_a_path_that_resolves_below_a_store_root() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join(".git/objects/aa")).expect("mkdir git store");
        fs::write(root.join(".git/objects/aa/deadbeef"), COMMITTED_OBJECT).expect("write object");

        // Aimed below the store root — the input on which the two predicates
        // disagree.
        std::os::unix::fs::symlink(root.join(".git/objects/aa"), root.join("shortcut"))
            .expect("symlink dir");
        // The same reach on the FILE arm, which never consulted a store
        // predicate at all: `is_secret_path` matches secret NAMES, and an
        // object file is named after its hash.
        std::os::unix::fs::symlink(root.join(".git/objects/aa/deadbeef"), root.join("blob.txt"))
            .expect("symlink file");

        // Wrong-refusal controls, so the fix cannot be "prune every link".
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        std::os::unix::fs::symlink(root.join("ok.txt"), root.join("alias.txt"))
            .expect("symlink ok");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let leaked: Vec<String> = payload
            .files
            .iter()
            .filter(|f| f.content.contains("COMMITTED-OBJECT"))
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            leaked.is_empty(),
            "a path resolving BELOW a VCS store root was attached — the walk's \
             self-test is not the deny walk's ancestor test: {leaked:?}"
        );
        assert_eq!(
            payload
                .files
                .iter()
                .filter(|f| f.content == "safe\n")
                .count(),
            2,
            "control: an ordinary file and an ordinary link to it must both stay attached"
        );
    }

    // ── D3 / core#335 c3 / D7 / core#339 c2: the escaping spellings, the
    //    blocking open, and the two scope lines nothing graded ─────────────

    /// An ordinary, non-denylisted body planted OUTSIDE the workspace, so a
    /// test can assert it never reached a payload without printing it.
    const OUTSIDE_BODY: &str = "PRIVATE-OUTSIDE-PAYLOAD 42\n";

    /// `mkfifo(3)`. There is no std equivalent and `libc` is already in this
    /// crate's graph (it is a dev-dependency on every platform).
    #[cfg(unix)]
    fn make_fifo(path: &Path) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;

        let c = CString::new(path.as_os_str().as_bytes()).expect("a path with no NUL");
        // SAFETY: `c` is a NUL-terminated path in a fresh temp directory.
        let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        assert_eq!(
            rc,
            0,
            "mkfifo {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }

    /// D3 — every `@dir` spelling the workspace root cannot lexically NAME
    /// resolved to a silently empty payload. `walk_dir` stripped each entry
    /// against `root`, got `None`, and `continue`d without even counting a
    /// skip, so the composer showed a successful chip carrying nothing. This
    /// `..` spelling names the workspace itself, so the workspace
    /// `.gitignore` still has jurisdiction — the core#335 property, for the
    /// walk rather than for `resolve_file`.
    #[test]
    fn a_dotdot_at_dir_attaches_the_tree_and_still_obeys_the_gitignore() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join("build")).expect("mkdir");
        fs::write(root.join(".gitignore"), "*.log\n").expect("write gitignore");
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        fs::write(root.join("build/out.log"), "build log\n").expect("write log");

        // Control: the plain spelling attaches the tree and honours the rule,
        // so neither assertion below can pass by the fixture being empty.
        let plain = resolve(&AtRef::parse("@./").expect("parse"), &root).expect("resolve");
        assert!(
            plain.files.iter().any(|f| f.content == "safe\n"),
            "fixture check: {plain:?}"
        );
        assert!(
            !plain.files.iter().any(|f| f.content == "build log\n"),
            "fixture check: {plain:?}"
        );

        let payload =
            resolve(&AtRef::parse("@../repo/").expect("parse"), &root).expect("resolve dir");
        assert!(
            payload.files.iter().any(|f| f.content == "safe\n"),
            "an escaping `@dir` spelling resolved to a silently empty payload: {payload:?}"
        );
        assert!(
            !payload.files.iter().any(|f| f.content == "build log\n"),
            "a spelling that resolves back into the workspace must still obey its \
             .gitignore: {payload:?}"
        );
    }

    /// D3, the absolute half. `@/abs/dir/` is the `@dir` twin of
    /// `an_absolute_path_outside_the_workspace_still_attaches` — the
    /// documented out-of-workspace attach — and it returned nothing at all,
    /// with no error and no warning. The walk is confined to the directory the
    /// reference NAMES, so core#339's scope guarantee still holds; it just
    /// holds about that directory rather than about a workspace the reference
    /// never mentioned.
    #[test]
    fn an_absolute_at_dir_outside_the_workspace_attaches_its_files() {
        let outside = TempDir::new().expect("tempdir");
        fs::write(outside.path().join("a.txt"), "alpha\n").expect("write a");
        fs::write(outside.path().join("b.txt"), "bravo\n").expect("write b");

        let tmp = TempDir::new().expect("tempdir");
        // The workspace's own rules have no jurisdiction out there — the same
        // answer `resolve_file` already gives for an absolute `@file`.
        fs::write(tmp.path().join(".gitignore"), "*.txt\n").expect("write gitignore");

        let payload = resolve(&AtRef::Dir(outside.path().to_path_buf()), tmp.path())
            .expect("an explicit absolute attach is a capability, not a bypass");
        let bodies: Vec<&str> = payload.files.iter().map(|f| f.content.as_str()).collect();
        assert!(
            bodies.contains(&"alpha\n") && bodies.contains(&"bravo\n"),
            "an absolute `@dir` outside the workspace resolved to an empty payload: {payload:?}"
        );
    }

    /// D7 — the walk OPENS every non-directory entry (`admit` →
    /// `same_file::Handle::from_path`), and opening a named pipe blocks until
    /// a writer appears. `@./` in any tree containing a FIFO — build systems,
    /// editors and language servers all leave them — wedged the turn forever
    /// inside a blocking syscall on the spawned turn task, where cancellation
    /// cannot reach it. `resolve_file` has always had the `is_file()` filter
    /// the walk lacked.
    #[cfg(unix)]
    #[test]
    fn the_at_dir_walk_does_not_block_on_a_fifo() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().to_path_buf();
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        make_fifo(&root.join("pipe"));
        // The denylisted spelling of the same shape. Before the file-type
        // filter, `admit()` ran BEFORE `is_secret_path`, so a FIFO named
        // `.env` blocked on the open before its name was ever judged — the
        // pre-core#339 walk skipped it by name without opening it.
        make_fifo(&root.join(".env"));

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let got = resolve(&AtRef::parse("@./").expect("parse"), &root);
            let _ = tx.send(got.map(|p| {
                (
                    p.files.iter().any(|f| f.content == "safe\n"),
                    p.files
                        .iter()
                        .any(|f| f.path.file_name().is_some_and(|n| n == ".env")),
                )
            }));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok((saw_ok, saw_env))) => {
                assert!(
                    saw_ok,
                    "the ordinary file next to the FIFO must still be attached"
                );
                assert!(!saw_env, "a FIFO named `.env` must not be read");
            }
            Ok(Err(e)) => panic!("the walk failed instead of skipping the FIFO: {e:?}"),
            Err(_) => panic!(
                "the @dir walk BLOCKED for 10s on a FIFO in the workspace — \
                 the turn is wedged, not slow"
            ),
        }
    }

    /// The sibling read site of the same class: `read_guarded` (the `@symbol`
    /// preview) also went straight to `admit` with no file-type filter, so a
    /// repomap-supplied path that happens to be a FIFO blocks the same way.
    /// A fix applied only to the walk leaves this one open.
    #[cfg(unix)]
    #[test]
    fn read_guarded_does_not_block_on_a_fifo() {
        let tmp = TempDir::new().expect("tempdir");
        let fifo = tmp.path().join("pipe");
        make_fifo(&fifo);

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_guarded(&fifo).is_err());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(refused) => assert!(refused, "a FIFO is not a readable @-reference target"),
            Err(_) => panic!(
                "read_guarded BLOCKED for 10s opening a FIFO — the same wedge as the @dir walk"
            ),
        }
    }

    /// core#335 c3, the ABSOLUTE half. `resolve_file` strips the resolved path
    /// against `canonical_root(root)`, not against `root`. When the workspace
    /// is reached through a link — a checkout under a symlinked path, a bind
    /// mount, macOS's `/var` -> `/private/var` — the two are different strings
    /// for the same directory, and stripping against the raw `root` returns
    /// `None` for every absolute spelling: the gitignore guard is never
    /// consulted and #335 reopens. The shipped suite had only a wrong-refusal
    /// control here, which passes on both arms.
    #[cfg(unix)]
    #[test]
    fn an_absolute_spelling_is_judged_against_the_canonical_workspace_root() {
        let tmp = TempDir::new().expect("tempdir");
        let parent = fs::canonicalize(tmp.path()).expect("canonicalize");
        let real = parent.join("real");
        fs::create_dir_all(real.join("build")).expect("mkdir");
        fs::write(real.join(".gitignore"), "*.log\n").expect("write gitignore");
        fs::write(real.join("ok.txt"), "safe\n").expect("write ok");
        fs::write(real.join("build/out.log"), "build log\n").expect("write log");
        // The workspace is handed to the resolver under a link, so `root` and
        // its canonical form differ.
        let link = parent.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        // Control: an ordinary file under the same root still attaches, so a
        // refusal below cannot be "this fixture root is unusable".
        let ok = resolve(&AtRef::File(real.join("ok.txt")), &link).expect("control attach");
        assert_eq!(ok.files[0].content, "safe\n");

        for spelled in [real.join("build/out.log"), link.join("build/out.log")] {
            let got = resolve(&AtRef::File(spelled.clone()), &link);
            assert!(
                matches!(got, Err(AtRefError::GitIgnored(_))),
                "an absolute spelling of a git-ignored workspace file must stay refused \
                 when the root is reached through a link ({}): got {got:?}",
                spelled.display()
            );
        }
    }

    /// core#339 c2 — the file arm of the scope check the ledger substituted for
    /// `symlink_metadata`, and which nothing graded. This is the mirror image
    /// of `at_dir_never_walks_a_symlink_into_a_credential_store`: the target is
    /// outside the workspace and its NAME is on no denylist, so
    /// `is_secret_path` cannot save it. Deleting
    /// `!admitted.canonical.starts_with(root_canonical)` left the whole suite
    /// green because every other symlink fixture points at a denylisted name.
    #[cfg(unix)]
    #[test]
    fn at_dir_never_inlines_an_ordinary_file_from_outside_the_workspace() {
        let outside = TempDir::new().expect("tempdir");
        let private = outside.path().join("taxes.txt");
        fs::write(&private, OUTSIDE_BODY).expect("write");

        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        std::os::unix::fs::symlink(&private, root.join("notes.txt")).expect("symlink");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("PRIVATE-OUTSIDE-PAYLOAD")),
            "the @dir walk inlined a file from outside the workspace through an in-root link"
        );
        assert!(
            payload.files.iter().any(|f| f.content == "safe\n"),
            "control: the ordinary file in the same directory must still be attached"
        );
    }

    /// core#339 c2 — the DIRECTORY arm of the same substitute, one line up the
    /// file. Its effect is not a leak (the file arm above catches the bytes);
    /// it is that the walk never LEAVES the workspace at all. The tell is the
    /// skip count: the link is one skipped entry, not the N files behind it.
    #[cfg(unix)]
    #[test]
    fn at_dir_does_not_descend_through_a_link_out_of_the_workspace() {
        let outside = TempDir::new().expect("tempdir");
        for i in 0..6 {
            fs::write(outside.path().join(format!("f{i}.txt")), OUTSIDE_BODY).expect("write");
        }

        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        std::os::unix::fs::symlink(outside.path(), root.join("docs")).expect("symlink");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let skipped = payload.warnings.iter().find_map(|w| match w {
            AtWarning::SkippedFiles { count } => Some(*count),
            _ => None,
        });
        assert_eq!(
            skipped,
            Some(1),
            "the walk descended through a link out of the workspace — it weighed the six \
             files behind the link instead of skipping the link itself: {payload:?}"
        );
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.content.contains("PRIVATE-OUTSIDE-PAYLOAD")),
            "an out-of-workspace file was attached: {payload:?}"
        );
        assert!(
            payload.files.iter().any(|f| f.content == "safe\n"),
            "control: the ordinary file must still be attached"
        );
    }

    /// The `SkippedFiles` count carried by a payload, if any.
    fn skipped_count(warnings: &[AtWarning]) -> Option<usize> {
        warnings.iter().find_map(|w| match w {
            AtWarning::SkippedFiles { count } => Some(*count),
            _ => None,
        })
    }

    // =======================================================================
    // FerroxLabs/wayland-core#377 c2 — no `continue` in `walk_dir` drops an
    // entry silently.
    // =======================================================================

    /// A pruned VCS store IS a drop, and is now counted.
    ///
    /// #322 c4 prunes `.git`/`.hg`/`.svn`/`.bzr` and their content stores from
    /// the walk: correct, and until now silent. The user asked for a directory
    /// and did not get all of it; c2's sentence is that no such `continue`
    /// leaves the payload without a `SkippedFiles` warning.
    #[test]
    fn a_pruned_vcs_store_is_counted_as_a_skipped_entry() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("ok.txt"), "safe").expect("write ok");
        fs::create_dir_all(root.join(".git/objects/ab")).expect("git");
        fs::write(root.join(".git/objects/ab/cd1234"), "blob").expect("obj");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");

        // WRONG-REFUSAL CONTROL: the ordinary file still attaches, so this is
        // not passing because the walk returned nothing.
        assert!(
            payload
                .files
                .iter()
                .any(|f| f.path.display().to_string().contains("ok.txt")),
            "control: the ordinary file must still attach: {:?}",
            payload.files
        );
        assert!(
            !payload
                .files
                .iter()
                .any(|f| f.path.display().to_string().contains("cd1234")),
            "control: the store must still be pruned"
        );
        assert_eq!(
            skipped_count(&payload.warnings),
            Some(1),
            "core#377 c2: pruning the VCS control directory dropped an entry \
             and must say so. Warnings: {:?}",
            payload.warnings
        );
    }

    /// The revisit guard is NOT a drop, and must not be counted.
    ///
    /// The other half of c2, and the reason it is a decision rather than a
    /// line: a directory reached a second time through a link back into the
    /// tree contributes its files ONCE, and they are in the payload. Counting
    /// it would report a skip for content the user did receive.
    #[cfg(unix)]
    #[test]
    fn a_directory_reached_twice_is_walked_once_and_not_counted_as_skipped() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join("real")).expect("real");
        fs::write(root.join("real/inner.txt"), "content").expect("inner");
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("link");

        let at = AtRef::parse("@./").expect("parse");
        let payload = resolve(&at, root).expect("resolve dir");

        let inner: Vec<_> = payload
            .files
            .iter()
            .filter(|f| f.path.display().to_string().contains("inner.txt"))
            .collect();
        assert_eq!(
            inner.len(),
            1,
            "control: the twice-reachable file must attach exactly once: {:?}",
            payload.files
        );
        assert_eq!(
            skipped_count(&payload.warnings),
            None,
            "core#377 c2: a directory reached twice is not a dropped entry — \
             its files are IN the payload. Warnings: {:?}",
            payload.warnings
        );
    }

    /// Does this line's CODE (not its comments) use `kw` as a keyword?
    ///
    /// Token boundaries on both sides, so `returns_early` and a `continue`
    /// inside a comment are not exits. Comments are stripped first because a
    /// gate that can be satisfied — or tripped — by prose grades prose.
    fn uses_keyword(line: &str, kw: &str) -> bool {
        let code = line.split("//").next().unwrap_or("");
        code.match_indices(kw).any(|(at, _)| {
            let before = code[..at].chars().next_back();
            let after = code[at + kw.len()..].chars().next();
            let boundary = |c: char| !c.is_alphanumeric() && c != '_';
            before.is_none_or(boundary) && after.is_none_or(boundary)
        })
    }

    /// THE SHAPE. Every construct in `walk_dir` that can end an iteration or
    /// the function either increments the skipped counter or carries an
    /// explicit `NOT A DROP:` justification.
    ///
    /// The previous pass closed the one `continue` the ticket named and left
    /// two others uncounted; the pass before that fixed three collapsed string
    /// literals and missed a fourth three lines below.
    ///
    /// **This gate was itself vacuous on its own subject until 2026-08-30, and
    /// a red arm is what proved it.** It matched `line.trim() == "continue;"`
    /// — one SPELLING of one exit. Which spellings a future author might use
    /// to drop an entry is an open alphabet, so a gate over spellings can
    /// always be walked around. MEASURED, not reasoned: replacing the counted
    /// `if !meta.is_file()` arm with `return Ok(());` drops every remaining
    /// entry in the directory with no `SkippedFiles` warning, compiles
    /// (`cargo check -p wcore-cli --tests` exit 0), is `cargo fmt --check`
    /// clean, and the old gate PASSED it.
    ///
    /// What ends an iteration is not an open alphabet. In Rust it is exactly
    /// `continue`, `break` and `return`, and asking "does this line use one of
    /// those three keywords" is decidable and total over the function. `?` is
    /// deliberately excluded: it propagates an `Err` the caller surfaces, so it
    /// is loud by construction — the opposite of the silence #377 is about.
    #[test]
    fn every_silent_exit_in_walk_dir_is_counted_or_justified() {
        const SOURCE: &str = include_str!("at_ref_resolve.rs");
        const EXITS: [&str; 3] = ["continue", "break", "return"];
        let lines: Vec<&str> = SOURCE.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.starts_with("fn walk_dir("))
            .expect("control: walk_dir must be in the scanned file");
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.starts_with("fn ") || l.starts_with("pub fn "))
            .map_or(lines.len(), |at| start + 1 + at);
        let body = &lines[start..end];

        let mut silent: Vec<String> = Vec::new();
        let mut total = 0usize;
        for (index, line) in body.iter().enumerate() {
            if !EXITS.iter().any(|kw| uses_keyword(line, kw)) {
                continue;
            }
            total += 1;
            // The counter must be the IMMEDIATELY preceding statement, not
            // merely somewhere above. A fixed look-back window is how the
            // `canon_for_scope` version of this gate was vacuous: the note
            // belonging to the arm NEXT DOOR satisfied it.
            let previous = body[..index]
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty() && !l.trim().starts_with("//"))
                .copied()
                .unwrap_or("");
            let counted = previous.trim() == "*skipped += 1;";
            // A justification, by contrast, is prose and lives in the comment
            // block directly above the arm.
            let justified = body[..index]
                .iter()
                .rev()
                .take_while(|l| {
                    l.trim().is_empty()
                        || l.trim().starts_with("//")
                        || l.trim().starts_with("if ")
                        || l.trim().starts_with("let ")
                        || l.trim().starts_with("*truncated")
                })
                .any(|l| l.contains("NOT A DROP:"));
            if !counted && !justified {
                silent.push(format!("line {}: {}", start + index + 1, line.trim()));
            }
        }
        // ANTI-VACUITY. Two separate ways this instrument can grade nothing:
        // pointed at the wrong function, or its keyword matcher silently
        // matching none. The second count is the one the old gate would have
        // failed — it saw 11 exits where there are 13.
        assert!(
            total >= 13,
            "control: the scan found only {total} loop/function exits in              walk_dir — the instrument is looking at the wrong function, or              `uses_keyword` has stopped matching"
        );
        // KNOWN-POSITIVE CONTROL on the matcher itself, in the same run. An
        // empty result reads as "no silent exits" and is the most common way
        // for a source-scanning gate to be wrong, so `uses_keyword` is made to
        // answer on a line of each shape — including the two it must REFUSE.
        for kw in EXITS {
            assert!(
                uses_keyword(&format!("            {kw};"), kw),
                "control: `uses_keyword` must match a bare `{kw}`"
            );
        }
        assert!(
            uses_keyword("            return Ok(());", "return")
                && uses_keyword("                continue 'entries;", "continue"),
            "control: `uses_keyword` must match an exit that carries a value              or a label"
        );
        assert!(
            !uses_keyword("            // continue here would drop it", "continue")
                && !uses_keyword("            let returned = f();", "return"),
            "control: `uses_keyword` must refuse a comment and an identifier,              or every line grades as an exit and `silent` is noise"
        );
        assert!(
            silent.is_empty(),
            "core#377 c2: these exits from walk_dir end an iteration or the              function without incrementing the skipped counter and without              saying why that is not a drop. Add `*skipped += 1;` or a              `NOT A DROP:` note:\n{}",
            silent.join("\n")
        );
    }

    /// THE OTHER HALF OF THE SHAPE: an entry can be dropped BEFORE it ever
    /// reaches the loop, and no gate over loop exits can see that.
    ///
    /// `walk_dir` read `entries.flatten().map(|e| e.path()).collect()`.
    /// `flatten()` silently discards every `Err` the `ReadDir` iterator yields
    /// — a dropped entry, no counter, no `SkippedFiles` warning, which is
    /// precisely the sentence core#377 c2 asserts. The gate above graded
    /// `continue` / `break` / `return` inside the loop and was structurally
    /// blind to it; MEASURED by the adversarial verifier, who added
    /// `.filter(|p| !p.to_string_lossy().ends_with(".txt"))` after the
    /// `flatten()` and watched it compile, pass `fmt --check` AND pass that
    /// gate.
    ///
    /// "Which iterator adapters can drop an element" is an open alphabet —
    /// `flatten`, `filter`, `filter_map`, `flat_map`, `take`, `take_while`,
    /// `skip`, `step_by`, `map_while`, `retain`, and whatever lands in a future
    /// `std`. A denylist over that alphabet can always be walked around, which
    /// is the same mistake the `continue`-spelling gate made.
    ///
    /// So this is an ALLOWLIST over the OCCURRENCES of the two bindings that
    /// carry an entry from `read_dir` to the loop that judges it. Each binding
    /// may be used only in the forms named below; every other use — an adapter,
    /// a `retain`, a `truncate`, a second `collect` — is a finding, whether or
    /// not anyone has thought of it. **Both bindings**, because closing only
    /// the first leaves the N+1 one line down: `paths.retain(..)` drops entries
    /// after the loop that fills it and before the loop that reads it.
    ///
    /// Decidable and total over the function, in the way
    /// `every_silent_exit_in_walk_dir_is_counted_or_justified` is decidable
    /// over its exits.
    #[test]
    fn every_entry_read_dir_yields_reaches_the_loop_or_the_counter() {
        const SOURCE: &str = include_str!("at_ref_resolve.rs");
        let lines: Vec<&str> = SOURCE.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.starts_with("fn walk_dir("))
            .expect("control: walk_dir must be in the scanned file");
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.starts_with("fn ") || l.starts_with("pub fn "))
            .map_or(lines.len(), |at| start + 1 + at);
        let body = &lines[start..end];

        // `code_uses` deliberately ignores comments: this very doc block names
        // `flatten()` and `retain(..)`, and a gate that graded prose would
        // grade its own explanation.
        let code_uses = |line: &str, name: &str| -> bool {
            let code = line.split("//").next().unwrap_or("");
            code.match_indices(name).any(|(at, _)| {
                let before = code[..at].chars().next_back();
                let after = code[at + name.len()..].chars().next();
                let boundary = |c: char| !c.is_alphanumeric() && c != '_';
                before.is_none_or(boundary) && after.is_none_or(boundary)
            })
        };

        // The two bindings, FOUND rather than assumed — a rename this test did
        // not follow must fail it, not silence it — each with the closed set of
        // forms it may appear in.
        let binding_of = |needle: &str| -> (usize, String) {
            let at = body
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("control: walk_dir must contain `{needle}`"));
            let name = body[at]
                .trim()
                .strip_prefix("let ")
                .and_then(|rest| rest.strip_prefix("mut ").or(Some(rest)))
                .and_then(|rest| rest.split([' ', ':', '=']).next())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| panic!("control: `{needle}` must bind a name"))
                .to_string();
            (at, name)
        };
        let (entries_at, entries) = binding_of("= fs::read_dir(");
        let (paths_at, paths) = binding_of("let mut paths: Vec<PathBuf>");

        let allowed = |line: &str, name: &str| -> bool {
            let code = line.trim();
            code == format!("for entry in {name} {{")
                || code == format!("for path in {name} {{")
                || code == format!("{name}.push(entry.path());")
                || code == format!("{name}.sort();")
        };

        let mut findings: Vec<String> = Vec::new();
        let mut uses = 0usize;
        for (index, line) in body.iter().enumerate() {
            for (binding_at, name) in [(entries_at, &entries), (paths_at, &paths)] {
                if !code_uses(line, name) {
                    continue;
                }
                uses += 1;
                if index == binding_at || allowed(line, name) {
                    continue;
                }
                findings.push(format!("line {}: {}", start + index + 1, line.trim()));
            }
        }

        // ANTI-VACUITY: each binding must be bound AND consumed, so four uses
        // is the floor. Fewer means the pipeline this test grades is gone and
        // it is scanning air.
        assert!(
            uses >= 4,
            "control: the scan found only {uses} uses of `{entries}` / \
             `{paths}` — the instrument is looking at the wrong function or a \
             binding was renamed"
        );
        // KNOWN-POSITIVE CONTROLS on the matcher, in the same run: the exact
        // line this defect WAS, the N+1 one line down, and the exact lines it
        // must be. An empty result reads as "no drops" and is the most common
        // way a source-scanning gate is wrong.
        for defect in [
            format!("        let paths = {entries}.flatten();"),
            format!("        {paths}.retain(|p| p.exists());"),
            format!("        {paths}.truncate(10);"),
        ] {
            let name = if code_uses(&defect, &entries) {
                &entries
            } else {
                &paths
            };
            assert!(
                code_uses(&defect, name) && !allowed(&defect, name),
                "control: the matcher must flag `{defect}`"
            );
        }
        for correct in [
            format!("    for entry in {entries} {{"),
            format!("        {paths}.push(entry.path());"),
            format!("    {paths}.sort();"),
            format!("    for path in {paths} {{"),
        ] {
            let name = if code_uses(&correct, &entries) {
                &entries
            } else {
                &paths
            };
            assert!(
                allowed(&correct, name),
                "control: the matcher must ACCEPT `{correct}`, or every correct \
                 shape grades as a finding"
            );
        }
        assert!(
            !code_uses(
                &format!("        // {entries}.flatten() drops errors"),
                &entries
            ),
            "control: the matcher must refuse a comment"
        );

        assert!(
            findings.is_empty(),
            "core#377 c2: an entry `read_dir` yielded is handled somewhere \
             other than the loop that counts drops. Every drop-capable form \
             (`flatten`, `filter`, `take`, `retain`, ...) discards entries \
             where no exit gate can see them and no `SkippedFiles` warning is \
             emitted. Move the handling inside the loop and count what it \
             drops:\n{}",
            findings.join("\n")
        );
    }
}
