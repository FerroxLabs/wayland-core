//! Guardrails for `@`-reference resolution: the secret denylist and the
//! `.gitignore` matcher.
//!
//! Both guardrails answer one question — *may this path be attached to a
//! message?* — and both err toward exclusion when uncertain, because the
//! cost of leaking a secret or an ignored artifact outweighs the cost of
//! a missed attachment the user can re-request explicitly. Split out of
//! `at_refs.rs` (W3-B) so parsing, completion, and resolution each import
//! only the guard surface they need.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use same_file::Handle;

// ─────────────────────────────────────────────────────────────────────────
// Secret denylist
// ─────────────────────────────────────────────────────────────────────────

/// Exact file names that are always treated as secrets, regardless of
/// directory.
const SECRET_FILENAMES: &[&str] = &[
    ".env",
    ".envrc",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".pgpass",
    "credentials",
    "credentials.json",
    "secrets.json",
    "secrets.yaml",
    "secrets.yml",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
];

/// File-name prefixes that mark a secret (`.env.local`, `.env.production`).
const SECRET_PREFIXES: &[&str] = &[".env."];

/// File-name suffixes that mark a secret regardless of stem.
const SECRET_SUFFIXES: &[&str] = &[
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".keystore",
    ".jks",
    "_rsa",
    "_ed25519",
];

/// True if `path` is on EITHER secret denylist. UX doc §3b: `@` "respects
/// the gitignore + a denylist (`.env`, key files) — never silently attach a
/// secret."
///
/// The union is the whole point. This module's file-name rules and
/// [`wcore_tools::workspace_policy::is_secret_path_static`] — the list
/// `Read`, `Grep` and `SecretDenyFs` enforce — had drifted apart in BOTH
/// directions, so neither is a superset of the other: nineteen credential
/// paths (`.git-credentials`, `.kube/config`, `.ssh/*`, `terraform.tfstate`,
/// …) were denied to the file tools yet attachable by `@`, and ten
/// (`.pgpass`, `.envrc`, `secrets.yml`, `*.jks`, …) only ever appeared here.
/// Consulting one list would have re-opened whichever half it dropped.
///
/// Two rule shapes, so two matching scopes:
///
/// * this module's rules match the FILE NAME (case-insensitively), so they
///   hold wherever the file lives;
/// * the shared list matches separator-anchored PATH FRAGMENTS (`/.ssh/`,
///   `/.git-credentials`), so a bare relative path misses every one of them.
///   Fourteen of the nineteen need the anchoring below to match at all.
///
/// Anchoring uses a synthetic root rather than the process CWD: it only ever
/// adds the leading separator the fragment rules need, and cannot import an
/// ambient directory that would deny an unrelated file. Purely lexical — this
/// runs inside the completion loop, on paths that need not exist.
pub fn is_secret_path(path: &Path) -> bool {
    if is_secret_file_name(path) {
        return true;
    }
    let anchored;
    let for_fragments = if path.is_absolute() {
        path
    } else {
        anchored = Path::new(std::path::MAIN_SEPARATOR_STR).join(path);
        anchored.as_path()
    };
    wcore_tools::workspace_policy::is_secret_path_static(for_fragments)
}

/// This module's own half of the union: the file-name rules
/// ([`SECRET_FILENAMES`] / [`SECRET_PREFIXES`] / [`SECRET_SUFFIXES`]).
/// Kept separate so the two halves stay individually readable — and so a
/// change to one is visibly a change to one.
fn is_secret_file_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();

    if SECRET_FILENAMES.iter().any(|s| *s == lower) {
        return true;
    }
    if SECRET_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    if SECRET_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        return true;
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────
// Target identity
// ─────────────────────────────────────────────────────────────────────────

/// An `@`-reference target resolved to exactly ONE filesystem object.
///
/// Every rule in this module judges a NAME — [`is_secret_path`] matches
/// file names and separator-anchored path fragments, [`GitIgnore`] matches
/// a relative path — while every read follows symlinks. A name is not an
/// identity, so the two disagreed: `@notes.txt`, where `notes.txt` is a
/// symlink to `~/.git-credentials`, satisfied every guard and then inlined
/// the credential store into the outgoing prompt (core#339).
///
/// This type is the join. It carries the handle a read must consume and
/// the canonical name the guards must judge, built so those two are
/// provably the same object.
#[derive(Debug)]
pub(super) struct ResolvedTarget {
    handle: Handle,
    canonical: PathBuf,
}

impl ResolvedTarget {
    /// The fully-resolved name of the object — no symlinks, no `..`, no
    /// route. Apply the guards to THIS, never to the name the user typed.
    pub(super) fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// Read the object's contents as UTF-8, from the handle opened during
    /// resolution.
    ///
    /// Takes `self` by value deliberately: once a caller has read the
    /// target it no longer holds anything it could re-resolve, so "guard
    /// one object, then read a path" is not expressible at this surface.
    pub(super) fn read_to_string(mut self) -> io::Result<String> {
        let mut buf = String::new();
        self.handle.as_file_mut().read_to_string(&mut buf)?;
        Ok(buf)
    }
}

/// Open `path` for reading in a way that cannot block on the open itself.
///
/// `File::open` on a FIFO with no writer — or on a blocking character
/// device such as a serial tty — waits inside the syscall, indefinitely.
/// The type check that refuses those objects runs on the handle, so it is
/// never reached: `@<fifo>` typed into the composer wedged the TUI on a
/// path where `Path::is_file()` would have answered instantly.
///
/// Unix answers that with `O_NONBLOCK`, which makes the open return for
/// every file type and is a no-op for the regular files this function is
/// actually after — POSIX gives `O_NONBLOCK` no effect on reads from a
/// regular file, so the later `read_to_string` is unchanged.
///
/// ## Windows
///
/// Windows has no equivalent flag. It is also a smaller exposure, but not
/// the zero one it was previously described as: no directory entry can BE a
/// pipe or a device, yet the `@`-surface accepts an absolute path, so a
/// device-namespace name (`\\.\pipe\…`) is typeable straight into the
/// composer. The cheap half of that is closed below by refusing the
/// device namespace before the open — a real file is never reached through
/// `\\.\`, so nothing a user owns can be refused by it.
///
/// What is deliberately NOT added is a reserved-name blocklist. On Windows
/// 11 build 26200 only a bare `NUL` still behaves as a device; `CON`,
/// `AUX`, `COM1` and `aux.txt` are ordinary files there, so the textbook
/// list would refuse real user data. `NUL` itself opens instantly and is
/// then refused by the handle's own type check, which returns
/// `FILE_TYPE_CHAR` for a device and `FILE_TYPE_PIPE` for a named pipe —
/// neither is `is_file()`.
///
/// NONE of this is measured. The `cfg(not(unix))` arm has been compiled and
/// clippy-checked for `x86_64-pc-windows-gnu` and never executed; the
/// residual risk is a `CreateFile` that blocks on a name this guard admits.
///
/// Every open in this module goes through here — that is what keeps the
/// "check the handle, not the name" ordering from costing a hang.
#[cfg(unix)]
fn open_without_blocking(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_without_blocking(path: &Path) -> io::Result<File> {
    use std::path::Prefix;

    // The device namespace, refused before the open rather than after it.
    // `Prefix::DeviceNS` is `\\.\…`; `Prefix::Verbatim` is a `\\?\` name
    // that is neither a drive nor a UNC share. Both are ways to reach an
    // object that is not a file at all, and neither can name a file the
    // user actually has.
    if let Some(Component::Prefix(p)) = path.components().next()
        && matches!(p.kind(), Prefix::DeviceNS(_) | Prefix::Verbatim(_))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an @-reference target must be a regular file, not a device name",
        ));
    }
    File::open(path)
}

/// Resolve `path` to the single object a guarded read will consume.
///
/// Refuses anything that is not a regular file, and refuses a target whose
/// identity moved while it was being resolved.
///
/// ## Why this is not canonicalize-then-open
///
/// Canonicalizing and then re-opening the canonical path leaves the very
/// race the guard exists to close: the check and the read become two
/// separate traversals, and whoever re-points the link — or renames a
/// secret over the canonical name — between them wins. Instead:
///
/// 1. open `path` once. This follows the link and pins a handle to one
///    object, and that handle is what the caller will read.
/// 2. canonicalize `path` to obtain a NAME with no symlinks left in it —
///    something the name-based guards can actually judge.
/// 3. open that name and require both handles to report the same
///    filesystem identity.
///
/// Step 3 is what binds the name to the bytes. If the link moved between
/// steps 1 and 2 the identities differ and the resolution is refused,
/// rather than guarding one file and quietly reading another.
///
/// The open in step 1 is deliberately non-blocking (see
/// [`open_without_blocking`]): the type check below judges the handle, and
/// a check that runs AFTER the open is worth nothing if the open itself
/// can sleep forever.
///
/// Note what is deliberately NOT done here: symlinks are not refused.
/// Repositories legitimately symlink real files, and a guard that blocks
/// the mechanism instead of the target removes a capability people rely
/// on — which is how guards end up switched off.
///
/// ## What this does not close
///
/// A HARD link. `ln ~/.git-credentials notes.txt` gives the secret a
/// second name that is itself already canonical, and no name-based rule
/// can tell the two apart because there is nothing for `canonicalize` to
/// unwind. Closing that needs an identity denylist rather than a path
/// denylist, which is a larger change than this one; it is written down
/// here so the boundary is visible rather than assumed away.
pub(super) fn resolve_target(path: &Path) -> io::Result<ResolvedTarget> {
    let file = open_without_blocking(path)?;
    if !file.metadata()?.is_file() {
        // A directory, a FIFO, a device. The refusal comes from the
        // handle's own metadata rather than from a `Path::is_file` stat:
        // opening on a directory succeeds on Unix and fails on Windows, so
        // only the handle reads the same on both, and only the handle
        // describes the object the read will actually consume.
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an @-reference target must be a regular file",
        ));
    }
    let handle = Handle::from_file(file)?;
    let canonical = fs::canonicalize(path)?;
    // Non-blocking here too: the canonical name is a second traversal, and
    // it can land on a FIFO the first one did not.
    if handle != Handle::from_file(open_without_blocking(&canonical)?)? {
        return Err(io::Error::other(
            "the @-reference target changed identity while it was being resolved",
        ));
    }
    Ok(ResolvedTarget { handle, canonical })
}

// ─────────────────────────────────────────────────────────────────────────
// Workspace-root helpers
// ─────────────────────────────────────────────────────────────────────────

/// The canonical form of a workspace root.
///
/// [`rel_to_root`] strips one path against another, so both sides have to
/// be in the same form or the strip fails, `rel_to_root` answers `None`,
/// and the caller skips the `.gitignore` verdict entirely (core#335).
/// A canonical target path therefore needs a canonical root — including on
/// macOS, where a temp dir is reached as `/var/…` but canonicalizes to
/// `/private/var/…`, and every gitignore check would otherwise be silently
/// skipped there while Linux CI stayed green.
///
/// Falls back to the root as given when it cannot be canonicalized (a root
/// that does not exist), which keeps the previous behaviour for that case
/// rather than failing a resolution the user can still complete.
pub(super) fn canonical_root(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// The path of `full` relative to `root`, as a `/`-joined string, if
/// `full` is inside `root`. Returns `None` for a path that escapes the
/// root (a `..` traversal or an unrelated absolute path) — such a path is
/// outside the gitignore's jurisdiction and is treated conservatively by
/// the caller.
pub(super) fn rel_to_root(full: &Path, root: &Path) -> Option<String> {
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

// ─────────────────────────────────────────────────────────────────────────
// .gitignore matching
// ─────────────────────────────────────────────────────────────────────────

/// A `.gitignore` rule set loaded from a project root.
///
/// Deliberately small: it covers the gitignore features that actually
/// matter for a *guardrail* — directory anchors, leading `/`, trailing `/`,
/// `*` / `?` wildcards, `**`, comments, and `!` negation. It does not aim
/// to be a bit-exact reimplementation of git's matcher; it errs toward
/// *excluding* a path when uncertain, which is the safe direction for a
/// "never attach a secret" guardrail.
#[derive(Debug, Default, Clone)]
pub struct GitIgnore {
    rules: Vec<IgnoreRule>,
}

#[derive(Debug, Clone)]
struct IgnoreRule {
    /// The pattern with anchoring/negation/trailing-slash markers stripped.
    pattern: String,
    /// `true` if this is a `!`-negation (re-include) rule.
    negated: bool,
    /// `true` if the pattern only matches directories (trailing `/`).
    dir_only: bool,
    /// `true` if the pattern is anchored to the gitignore's directory
    /// (a leading `/`, or an interior `/`).
    anchored: bool,
}

impl GitIgnore {
    /// Load `.gitignore` from `root`. A missing file yields an empty
    /// (matches-nothing) rule set — the common case for a sub-directory.
    pub fn load(root: &Path) -> Self {
        let path = root.join(".gitignore");
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Parse `.gitignore` text into a rule set.
    pub fn parse(text: &str) -> Self {
        let mut rules = Vec::new();
        for raw in text.lines() {
            let line = raw.trim_end();
            // Blank lines and comments are skipped. A literal `#` can be
            // escaped as `\#`; we honor that minimally.
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut pat = line;
            let negated = pat.starts_with('!');
            if negated {
                pat = &pat[1..];
            }
            if let Some(stripped) = pat.strip_prefix('\\') {
                // `\#…` / `\!…` — the escape just protects the first char.
                pat = stripped;
            }
            let dir_only = pat.ends_with('/');
            let pat = pat.trim_end_matches('/');
            // Anchored if it begins with `/` or contains an interior `/`.
            let interior_slash = pat.trim_start_matches('/').contains('/');
            let anchored = pat.starts_with('/') || interior_slash;
            let pattern = pat.trim_start_matches('/').to_string();
            if pattern.is_empty() {
                continue;
            }
            rules.push(IgnoreRule {
                pattern,
                negated,
                dir_only,
                anchored,
            });
        }
        Self { rules }
    }

    /// True if `rel` (a path relative to the gitignore's directory, using
    /// `/` separators) is ignored. `is_dir` lets directory-only rules
    /// (`build/`) apply correctly.
    ///
    /// Later rules win — git's last-match-wins semantics — so a `!`
    /// negation after a broad ignore re-includes the path.
    pub fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        let rel = rel.trim_start_matches('/');
        let mut ignored = false;
        for rule in &self.rules {
            if rule.dir_only && !is_dir {
                continue;
            }
            if rule.matches(rel) {
                ignored = !rule.negated;
            }
        }
        ignored
    }

    /// The number of parsed rules — used by tests to assert comment/blank
    /// stripping.
    #[cfg(test)]
    pub(super) fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

impl IgnoreRule {
    /// True if this rule matches the relative path `rel`.
    fn matches(&self, rel: &str) -> bool {
        if self.anchored {
            glob_match(&self.pattern, rel)
        } else {
            // An unanchored rule matches the path's basename OR any
            // trailing path segment — git applies a non-anchored pattern
            // at every directory level.
            if glob_match(&self.pattern, rel) {
                return true;
            }
            rel.split('/').any(|seg| glob_match(&self.pattern, seg))
                || rel
                    .match_indices('/')
                    .any(|(i, _)| glob_match(&self.pattern, &rel[i + 1..]))
        }
    }
}

/// Glob match supporting `*` (any run within a segment), `**` (any run
/// across segments), and `?` (one char). Anchored at both ends.
///
/// Recursive with a tight branching factor — gitignore patterns are short,
/// so the worst case is bounded in practice.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_inner(&p, &t)
}

fn glob_inner(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') => {
            // `**` — match across `/`. `*` — match within a segment only.
            let double = p.get(1) == Some(&'*');
            let rest = if double { &p[2..] } else { &p[1..] };
            // Skip a `/` that immediately follows `**` so `**/foo` matches
            // `foo` at the root too.
            let rest = if double && rest.first() == Some(&'/') {
                &rest[1..]
            } else {
                rest
            };
            if glob_inner(rest, t) {
                return true;
            }
            for (i, &c) in t.iter().enumerate() {
                if !double && c == '/' {
                    break;
                }
                if glob_inner(rest, &t[i + 1..]) {
                    return true;
                }
            }
            false
        }
        Some('?') => match t.first() {
            Some(&c) if c != '/' => glob_inner(&p[1..], &t[1..]),
            _ => false,
        },
        Some(&pc) => match t.first() {
            Some(&tc) if tc == pc => glob_inner(&p[1..], &t[1..]),
            _ => false,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────
// The admission gate — the ONE place `@`-reference policy is decided
// ─────────────────────────────────────────────────────────────────────────

/// Why [`AtGate`] refused a candidate.
///
/// A typed verdict rather than a bool: each consumer renders the refusal in
/// its own vocabulary (the resolver into [`AtRefError`], the walk into a
/// skip counter, the popup into "not offered"), and none of them may decide
/// *whether* to refuse.
#[derive(Debug)]
pub(super) enum Refusal {
    /// The typed name, or the object it resolves to, is on the secret
    /// denylist.
    Secret,
    /// The typed name, or the canonical name, is git-ignored.
    GitIgnored,
    /// The candidate was DISCOVERED inside the workspace but the object it
    /// resolves to lives outside it.
    EscapesRoot,
    /// The candidate could not be resolved to a readable regular file — a
    /// dangling link, a device, a FIFO, a directory, a permission failure,
    /// or an identity that moved mid-resolution.
    Unresolvable(io::Error),
}

/// How a candidate reached the gate.
///
/// This is the ONLY policy difference between the `@`-reference consumers,
/// and it exists so that difference is *named* rather than re-discovered as
/// a missing check in whichever consumer was written last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    /// The user NAMED this object: `@file`, or the popup row that is about
    /// to become one. Its label in the payload is the name the user typed,
    /// so a link that leaves the workspace is not a substitution — it is the
    /// capability repositories actually use (`docs/spec.md ->
    /// ../shared/spec.md`), and `a_symlink_to_an_ordinary_file_still_resolves`
    /// pins it. Every other rule still applies to the object it points at.
    Named,
    /// The `@dir` walk DISCOVERED this object; nobody named it. It is
    /// reported under a root-relative label the walk synthesises, so an
    /// out-of-tree body would arrive wearing an in-root name — precisely the
    /// substitution the guard exists to stop (core#339). Containment is
    /// therefore required here and only here.
    Walked,
}

/// The single gate every `@`-reference consumer passes a candidate through.
///
/// Before this type existed, `resolve_file`, `walk_dir` and the completion
/// popup each carried their own hand-assembled copy of the same four rules —
/// containment, regular-file identity, the secret denylist, and `.gitignore`
/// under both the lexical and the canonical name. Two of the three were
/// updated when the rules were tightened and the third silently kept the
/// weaker set: the walk applied only the lexical `.gitignore` verdict and no
/// containment at all, so `@./` inlined an out-of-workspace body under an
/// in-root path, and inlined a body `@notable.txt` was refusing by name.
///
/// The rules therefore live here, once. A consumer chooses only which
/// *entry point* fits what it is about to do — read one object, walk one
/// directory, or offer a row — and cannot choose which rules apply.
pub(super) struct AtGate<'a> {
    /// The workspace root as the caller gave it — the form the *typed* name
    /// is relative to.
    root: &'a Path,
    /// The canonical workspace root — the form a *canonical* target must be
    /// measured against. On macOS these two differ for every temp dir
    /// (`/var/…` vs `/private/var/…`), which is why both are kept.
    croot: PathBuf,
    /// The `.gitignore` rule set loaded from `root`.
    ignore: &'a GitIgnore,
}

impl<'a> AtGate<'a> {
    /// Build a gate for one resolution against `root`.
    pub(super) fn new(root: &'a Path, ignore: &'a GitIgnore) -> Self {
        Self {
            root,
            croot: canonical_root(root),
            ignore,
        }
    }

    /// The workspace root as the caller gave it — the form a payload label
    /// is built relative to.
    pub(super) fn root(&self) -> &Path {
        self.root
    }

    /// Admit a candidate that is about to be READ, returning the bound
    /// target the read must consume.
    ///
    /// This is the only entry point that binds a handle, and it is the only
    /// one that can: the canonical name the guards judge and the bytes the
    /// caller reads are the same object by construction (see
    /// [`resolve_target`]).
    pub(super) fn admit_file(
        &self,
        lexical: &Path,
        reach: Reach,
    ) -> Result<ResolvedTarget, Refusal> {
        self.judge_lexical(lexical, false)?;
        let target = resolve_target(lexical).map_err(Refusal::Unresolvable)?;
        self.judge_canonical(target.canonical(), false, reach)?;
        Ok(target)
    }

    /// Admit a directory the `@dir` walk is about to descend into — the walk
    /// root included. Returns the canonical directory path, which the caller
    /// uses as the walk's visited-set key.
    ///
    /// Always [`Reach::Walked`]: every file the walk emits from underneath
    /// this directory is labelled relative to the workspace root, so a
    /// directory that resolves outside the workspace would launder its whole
    /// subtree into in-root-looking names.
    pub(super) fn admit_dir(&self, lexical: &Path) -> Result<PathBuf, Refusal> {
        self.judge_lexical(lexical, true)?;
        let canonical = fs::canonicalize(lexical).map_err(Refusal::Unresolvable)?;
        self.judge_canonical(&canonical, true, Reach::Walked)?;
        Ok(canonical)
    }

    /// Admit a candidate the completion popup is about to OFFER.
    ///
    /// Deliberately does not open anything: offering a row reads no bytes,
    /// this runs inside the keystroke loop, and a candidate whose target
    /// cannot be canonicalized (a broken link) leaks nothing and still
    /// deserves to be listed. The authoritative, race-free verdict is the
    /// one [`admit_file`](Self::admit_file) takes at resolution time; this
    /// pass exists so the popup never offers a row the resolver will refuse.
    /// It is [`Reach::Named`] for exactly that reason — a popup stricter
    /// than the resolver is the same disagreement with the sign flipped.
    pub(super) fn admit_offer(&self, lexical: &Path, is_dir: bool) -> Result<(), Refusal> {
        self.judge_lexical(lexical, is_dir)?;
        if let Ok(canonical) = fs::canonicalize(lexical) {
            self.judge_canonical(&canonical, is_dir, Reach::Named)?;
        }
        Ok(())
    }

    /// The rules that need only the name as typed or listed. Purely lexical,
    /// so it is cheap enough for the completion loop and keeps `@.env` a
    /// refusal even when no such file exists.
    fn judge_lexical(&self, lexical: &Path, is_dir: bool) -> Result<(), Refusal> {
        if is_secret_path(lexical) {
            return Err(Refusal::Secret);
        }
        if let Some(rel) = rel_to_root(lexical, self.root)
            && self.ignore.is_ignored(&rel, is_dir)
        {
            return Err(Refusal::GitIgnored);
        }
        Ok(())
    }

    /// The rules that need the resolved identity of the object.
    ///
    /// The `.gitignore` verdict here is ADDITIVE, never substitutive: a
    /// candidate has the relative path it was typed as *and* the one its
    /// target canonicalizes to, and either being ignored is a refusal.
    /// Substituting the canonical verdict for the lexical one un-ignores
    /// every lexically-ignored link; dropping the canonical verdict — which
    /// is what the walk did — lets a link launder an ignored file under an
    /// innocent name.
    fn judge_canonical(&self, canonical: &Path, is_dir: bool, reach: Reach) -> Result<(), Refusal> {
        if reach == Reach::Walked && !canonical.starts_with(&self.croot) {
            return Err(Refusal::EscapesRoot);
        }
        if is_secret_path(canonical) {
            return Err(Refusal::Secret);
        }
        if let Some(rel) = rel_to_root(canonical, &self.croot)
            && self.ignore.is_ignored(&rel, is_dir)
        {
            return Err(Refusal::GitIgnored);
        }
        Ok(())
    }
}

/// True if `path` is a directory once symlinks are followed.
///
/// Shared by the walk and the completion popup so the two surfaces cannot
/// disagree about what an entry IS. `fs::symlink_metadata` — and
/// `DirEntry::file_type`, which is the same stat — call a symlink-to-a-
/// directory a non-directory, which is how `@alias/` resolved while `@./`
/// silently omitted the same object. `stat` does not open, so this is safe
/// on a FIFO or a device; a dangling link answers `false` and falls to the
/// file branch, where the identity check refuses it.
pub(super) fn is_dir_following_links(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── secret denylist ──────────────────────────────────────────────────

    #[test]
    fn secret_denylist_blocks_env_and_keys() {
        assert!(is_secret_path(Path::new(".env")));
        assert!(is_secret_path(Path::new("project/.env")));
        assert!(is_secret_path(Path::new(".env.production")));
        assert!(is_secret_path(Path::new("config/server.pem")));
        assert!(is_secret_path(Path::new("id_rsa")));
        assert!(is_secret_path(Path::new("certs/tls.key")));
        assert!(is_secret_path(Path::new("CREDENTIALS.JSON"))); // case-insensitive

        assert!(!is_secret_path(Path::new("src/main.rs")));
        assert!(!is_secret_path(Path::new("README.md")));
        assert!(!is_secret_path(Path::new("environment.rs")));
    }

    /// Every path on which the `@`-attach denylist and the `wcore-tools`
    /// workspace-policy denylist DIVERGED before they were unioned.
    ///
    /// Nineteen are carried only by `workspace_policy::is_secret_path_static`
    /// — the list `Read`, `Grep` and `SecretDenyFs` already enforce — and were
    /// invisible to this module's file-name rules. Ten are carried only by
    /// this module's rules. Neither list is a superset of the other, which is
    /// exactly why the guard has to consult BOTH.
    ///
    /// Every entry here is denied by exactly ONE of the two lists, so this
    /// table goes red if EITHER of them loses an entry. No single-list test
    /// can have that property, and its absence is what let the two lists
    /// drift apart in the first place.
    const DIVERGENT_SECRET_PATHS: &[&str] = &[
        // ── carried only by wcore-tools' workspace policy ────────────────
        ".git-credentials",
        ".git/config",
        ".git/hooks/pre-commit",
        ".hg/hgrc",
        ".dockercfg",
        ".docker/config.json",
        ".kube/config",
        ".ssh/config",
        ".ssh/known_hosts",
        ".gnupg/secring.gpg",
        ".aws/config",
        ".azure/accessTokens.json",
        ".gcloud/credentials.db",
        "gradle.properties",
        "terraform.tfstate",
        "terraform.tfstate.backup",
        "service-account.json",
        "key.json",
        "gcp-key.json",
        // ── carried only by this module's file-name rules ────────────────
        ".pgpass",
        ".envrc",
        "secrets.json",
        "secrets.yaml",
        "secrets.yml",
        "credentials.json",
        "release.keystore",
        "signing.jks",
        "deploy_rsa",
        "deploy_ed25519",
    ];

    /// Ordinary files that must stay attachable. Without this control the
    /// table above would be satisfied by a guard that denies everything.
    /// `turnkey.json` / `monkey.json` additionally pin the `*-key.json`
    /// rule's separator boundary.
    const ATTACHABLE_PATHS: &[&str] = &[
        "src/main.rs",
        "README.md",
        "Cargo.toml",
        "environment.rs",
        "config",
        "notes/turnkey.json",
        "docs/monkey.json",
    ];

    #[test]
    fn the_attach_guard_denies_every_path_either_denylist_carries() {
        let escaped: Vec<&str> = DIVERGENT_SECRET_PATHS
            .iter()
            .copied()
            .filter(|p| !is_secret_path(Path::new(p)))
            .collect();
        assert!(
            escaped.is_empty(),
            "these secret paths would be attached to a prompt: {escaped:?}"
        );

        let refused: Vec<&str> = ATTACHABLE_PATHS
            .iter()
            .copied()
            .filter(|p| is_secret_path(Path::new(p)))
            .collect();
        assert!(
            refused.is_empty(),
            "ordinary files must stay attachable, but these were denied: {refused:?}"
        );
    }

    // ── gitignore ────────────────────────────────────────────────────────

    #[test]
    fn gitignore_basic_patterns() {
        let gi = GitIgnore::parse("target/\n*.log\n/build\nnode_modules\n");
        assert!(gi.is_ignored("target", true));
        assert!(gi.is_ignored("crates/foo/target", true));
        assert!(!gi.is_ignored("target", false)); // dir-only rule
        assert!(gi.is_ignored("debug.log", false));
        assert!(gi.is_ignored("logs/run.log", false));
        assert!(gi.is_ignored("build", false)); // anchored at root
        assert!(!gi.is_ignored("crates/build", false)); // anchored — not nested
        assert!(gi.is_ignored("node_modules", true));
        assert!(gi.is_ignored("pkg/node_modules", true));
        assert!(!gi.is_ignored("src/main.rs", false));
    }

    #[test]
    fn gitignore_negation_re_includes() {
        let gi = GitIgnore::parse("*.log\n!keep.log\n");
        assert!(gi.is_ignored("debug.log", false));
        assert!(!gi.is_ignored("keep.log", false)); // negation wins (last match)
    }

    #[test]
    fn gitignore_comments_and_blank_lines_are_skipped() {
        let gi = GitIgnore::parse("# a comment\n\n  \n*.tmp\n");
        assert!(gi.is_ignored("x.tmp", false));
        assert_eq!(gi.rule_count(), 1);
    }

    #[test]
    fn gitignore_double_star_crosses_directories() {
        let gi = GitIgnore::parse("**/generated/*.rs\n");
        assert!(gi.is_ignored("a/b/generated/x.rs", false));
        assert!(gi.is_ignored("generated/x.rs", false));
        assert!(!gi.is_ignored("generated/x.txt", false));
    }

    // ── target identity ──────────────────────────────────────────────────

    /// The identity primitive itself, on every platform: a path routed
    /// through `..` resolves to one canonical name, and the bytes come from
    /// that same object.
    #[test]
    fn resolve_target_reports_the_canonical_name_and_reads_that_object() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = fs::canonicalize(tmp.path()).expect("canonical root");
        fs::create_dir(root.join("sub")).expect("mkdir sub");
        fs::write(root.join("sub").join("a.txt"), "body").expect("write");

        let routed = root.join("sub").join("..").join("sub").join("a.txt");
        let target = resolve_target(&routed).expect("resolve");
        assert_eq!(target.canonical(), root.join("sub").join("a.txt"));
        assert_eq!(target.read_to_string().expect("read"), "body");
    }

    /// A directory is not a readable target. `File::open` on a directory
    /// succeeds on Unix and fails on Windows, so the refusal has to come
    /// from the handle's own metadata to be the same on both.
    #[test]
    fn resolve_target_refuses_a_directory() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        assert!(resolve_target(tmp.path()).is_err());
    }

    #[test]
    fn resolve_target_refuses_a_missing_path() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let err = resolve_target(&tmp.path().join("nope.txt")).expect_err("missing");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    /// A dangling symlink has no target to guard, so it must refuse rather
    /// than fall back to judging the link's own name.
    #[cfg(unix)]
    #[test]
    fn resolve_target_refuses_a_dangling_symlink() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::os::unix::fs::symlink(tmp.path().join("gone"), tmp.path().join("link.txt"))
            .expect("symlink");
        assert!(resolve_target(&tmp.path().join("link.txt")).is_err());
    }

    /// `canonical_root` must not turn a root that cannot be canonicalized
    /// into an empty or absolute-root path — the gitignore jurisdiction
    /// check depends on it still naming the caller's root.
    #[test]
    fn canonical_root_falls_back_to_the_given_root() {
        let missing = Path::new("relative/does/not/exist");
        assert_eq!(canonical_root(missing), missing.to_path_buf());
    }

    /// Pins the ASSUMPTION `resolve_target` rests on: that `Handle`
    /// equality is object identity and not something weaker. If it agreed
    /// across distinct objects the guard would be vacuous; if it disagreed
    /// across the same object every ordinary read would be refused. This
    /// grades the assumption, not the dependency.
    #[test]
    fn handle_equality_is_object_identity() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, "a").expect("write a");
        fs::write(&b, "b").expect("write b");

        assert_eq!(
            Handle::from_path(&a).expect("handle"),
            Handle::from_path(&a).expect("handle"),
            "two handles on one object must agree"
        );
        assert_ne!(
            Handle::from_path(&a).expect("handle"),
            Handle::from_path(&b).expect("handle"),
            "two distinct objects must not share an identity"
        );
    }

    /// The read must come from the handle pinned during resolution, not
    /// from the path. Re-pointing the link after the guard and before the
    /// read is the deterministic form of the race that
    /// canonicalize-then-reopen loses: a path-based read returns the NEW
    /// target's bytes under the OLD target's canonical name, which is
    /// precisely "guard one object, read another".
    #[cfg(unix)]
    #[test]
    fn resolve_target_reads_the_handle_it_pinned_not_the_path() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = fs::canonicalize(tmp.path()).expect("canonical root");
        fs::write(root.join("first.txt"), "first body").expect("write first");
        fs::write(root.join("second.txt"), "second body").expect("write second");
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(root.join("first.txt"), &link).expect("symlink");

        let target = resolve_target(&link).expect("resolve");
        assert_eq!(target.canonical(), root.join("first.txt"));

        // Swap a DIFFERENT object in at the canonical name, between the
        // guard and the read. A rename replaces the directory entry, so the
        // name now points at a new inode while the pinned handle still holds
        // the old one — exactly the window a path-based read loses.
        fs::rename(root.join("second.txt"), root.join("first.txt")).expect("rename over");

        assert_eq!(
            target.read_to_string().expect("read"),
            "first body",
            "the read followed the path instead of the handle it guarded"
        );
    }

    // ── target identity: the open must not block ─────────────────────────

    /// A named pipe with no writer — the cheapest object whose
    /// `open(O_RDONLY)` blocks inside the syscall.
    #[cfg(unix)]
    fn make_fifo(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("fifo path");
        let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo: {}", std::io::Error::last_os_error());
    }

    /// Run `resolve_target` on another thread and refuse to wait forever.
    /// `None` means it never returned — the failure this test exists for,
    /// expressed as a failing assertion rather than a hung harness.
    #[cfg(unix)]
    fn resolve_within(path: &Path, secs: u64) -> Option<std::io::Result<()>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let owned = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(resolve_target(&owned).map(|_| ()));
        });
        rx.recv_timeout(std::time::Duration::from_secs(secs)).ok()
    }

    /// A FIFO must be refused, and refused *without the open blocking*.
    ///
    /// The type check has to run on a handle that cannot wait. Opening
    /// first and asking the handle for its type afterwards never reaches
    /// the refusal at all: `open(O_RDONLY)` on a writer-less FIFO — or on
    /// a blocking character device such as a serial tty — sleeps in the
    /// kernel, so `@<fifo>` typed into the composer wedges the TUI with no
    /// way back.
    #[cfg(unix)]
    #[test]
    fn resolve_target_refuses_a_fifo_without_blocking_on_the_open() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let fifo = tmp.path().join("pipe");
        make_fifo(&fifo);
        let control = tmp.path().join("plain.txt");
        fs::write(&control, "ordinary").expect("write control");

        // Positive control on the same binary, same call, same deadline: a
        // regular file returns. A timeout below is therefore the FIFO and
        // not a harness that never lets anything finish.
        assert!(
            resolve_within(&control, 5)
                .expect("the control blocked")
                .is_ok(),
            "the control file must resolve"
        );

        let verdict = resolve_within(&fifo, 5).expect(
            "resolve_target never returned on a FIFO — the open blocked before the type check",
        );
        assert!(verdict.is_err(), "a FIFO is not a regular file");
    }

    // ── the admission gate ───────────────────────────────────────────────

    /// `Reach` is the ONLY policy difference the gate permits between
    /// consumers, so it gets a test that shows both sides of it at once on
    /// the same object. If a future edit collapses the two reaches, one of
    /// these two assertions goes red whichever way it collapses.
    #[cfg(unix)]
    #[test]
    fn the_gate_splits_a_named_target_from_a_walked_one_only_on_containment() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path().join("ws");
        let outside = tmp.path().join("home");
        fs::create_dir_all(&root).expect("mkdir ws");
        fs::create_dir_all(&outside).expect("mkdir home");
        fs::write(outside.join("shared.md"), "shared body").expect("write");
        let link = root.join("link.md");
        std::os::unix::fs::symlink(outside.join("shared.md"), &link).expect("symlink");

        let ignore = GitIgnore::default();
        let gate = AtGate::new(&root, &ignore);

        let named = gate
            .admit_file(&link, Reach::Named)
            .expect("a NAMED out-of-tree target is a capability, not a leak");
        assert_eq!(named.read_to_string().expect("read"), "shared body");

        let walked = gate
            .admit_file(&link, Reach::Walked)
            .expect_err("a WALKED target must not leave the workspace");
        assert!(matches!(walked, Refusal::EscapesRoot), "got {walked:?}");
    }

    /// The canonical `.gitignore` verdict, at the gate rather than in one
    /// consumer's copy of it. This is D2 reduced to the gate: `notable.txt`
    /// is not ignored by name, its target is, and the gate refuses it under
    /// BOTH reaches — there is no reach in which the canonical verdict is
    /// optional.
    #[cfg(unix)]
    #[test]
    fn the_gate_applies_the_gitignore_verdict_to_the_canonical_name_too() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("artifact.txt"), "ignored body").expect("artifact");
        fs::write(root.join("plain.txt"), "plain body").expect("plain");
        let link = root.join("notable.txt");
        std::os::unix::fs::symlink(root.join("artifact.txt"), &link).expect("symlink");

        let ignore = GitIgnore::parse("artifact.txt\n");
        let gate = AtGate::new(root, &ignore);

        // Control: an ordinary in-root file is admitted under both reaches,
        // so the refusals below are the rule and not the fixture.
        for reach in [Reach::Named, Reach::Walked] {
            gate.admit_file(&root.join("plain.txt"), reach)
                .unwrap_or_else(|e| panic!("control refused under {reach:?}: {e:?}"));
        }
        for reach in [Reach::Named, Reach::Walked] {
            let err = gate
                .admit_file(&link, reach)
                .expect_err("a link to an ignored file must be refused");
            assert!(
                matches!(err, Refusal::GitIgnored),
                "under {reach:?}: got {err:?}"
            );
        }
    }

    /// `is_dir_following_links` is what makes the two `@`-surfaces agree
    /// about what an entry IS, so it is pinned directly: a symlink to a
    /// directory is a directory, a symlink to a file is not, and neither a
    /// dangling link nor a FIFO answers `true` (or blocks — `stat` does not
    /// open).
    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_is_classified_as_a_directory() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir(root.join("real")).expect("mkdir");
        fs::write(root.join("file.txt"), "body").expect("write");
        std::os::unix::fs::symlink(root.join("real"), root.join("dirlink")).expect("dirlink");
        std::os::unix::fs::symlink(root.join("file.txt"), root.join("filelink")).expect("filelink");
        std::os::unix::fs::symlink(root.join("nope"), root.join("dangling")).expect("dangling");
        let fifo = root.join("pipe");
        let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).expect("cstr");
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");

        assert!(is_dir_following_links(&root.join("real")));
        assert!(is_dir_following_links(&root.join("dirlink")));
        assert!(!is_dir_following_links(&root.join("file.txt")));
        assert!(!is_dir_following_links(&root.join("filelink")));
        assert!(!is_dir_following_links(&root.join("dangling")));
        assert!(!is_dir_following_links(&fifo));
    }

    // ══ REFUTATION PROBES (external auditor) ═════════════════════════════

    /// A REAL blocking character device, not just a FIFO.
    ///
    /// Opt-in: set `REFUT_BLOCKING_CHARDEV` to a device whose plain
    /// `open(O_RDONLY)` blocks (e.g. `/dev/ttyS1` after `stty -clocal`).
    /// The test proves the device blocks a PLAIN open first, so a pass
    /// cannot come from a device that never blocked.
    #[cfg(unix)]
    #[test]
    fn refut_resolve_target_refuses_a_blocking_character_device() {
        let Ok(dev) = std::env::var("REFUT_BLOCKING_CHARDEV") else {
            eprintln!("REFUT: skipped, REFUT_BLOCKING_CHARDEV unset");
            return;
        };
        let dev = PathBuf::from(dev);

        // Precondition: a PLAIN open really does block on this device.
        let (tx, rx) = std::sync::mpsc::channel();
        let d2 = dev.clone();
        std::thread::spawn(move || {
            let _ = tx.send(File::open(&d2).map(|_| ()));
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).is_err(),
            "PRECONDITION FAILED: a plain open on {dev:?} did not block, so this test proves nothing"
        );

        // Positive control on the same binary/call/deadline.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let control = tmp.path().join("plain.txt");
        fs::write(&control, "ordinary").expect("write control");
        assert!(
            resolve_within(&control, 5)
                .expect("the control blocked")
                .is_ok(),
            "the control file must resolve"
        );

        let verdict =
            resolve_within(&dev, 5).expect("REFUTED: resolve_target blocked on a character device");
        assert!(verdict.is_err(), "a character device is not a regular file");
    }

    /// O_NONBLOCK must not turn a legitimate regular file into a refusal
    /// or a short read. 8 MiB, read back byte-for-byte.
    #[cfg(unix)]
    #[test]
    fn refut_a_large_regular_file_still_reads_whole_under_o_nonblock() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let big = tmp.path().join("big.txt");
        let body: String = "abcdefghijklmnopqrstuvwxyz0123456789\n".repeat(230_000);
        fs::write(&big, &body).expect("write big");
        let t = resolve_target(&big).expect("REFUTED: a large regular file was refused");
        let got = t
            .read_to_string()
            .expect("REFUTED: read failed under O_NONBLOCK");
        assert_eq!(got.len(), body.len(), "short read under O_NONBLOCK");
        assert_eq!(got, body, "corrupt read under O_NONBLOCK");
    }

    /// A regular file whose data arrives slowly (written by a concurrent
    /// writer while the read is in flight) must not produce EAGAIN.
    #[cfg(unix)]
    #[test]
    fn refut_a_slow_regular_file_is_not_spuriously_refused() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let slow = tmp.path().join("slow.txt");
        fs::write(&slow, "seed\n").expect("seed");
        let w = slow.clone();
        let h = std::thread::spawn(move || {
            use std::io::Write;
            for i in 0..40 {
                let mut f = fs::OpenOptions::new()
                    .append(true)
                    .open(&w)
                    .expect("append");
                writeln!(f, "line {i}").expect("write");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        let t = resolve_target(&slow).expect("REFUTED: a slow regular file was refused");
        let got = t
            .read_to_string()
            .expect("REFUTED: EAGAIN on a regular file");
        h.join().expect("writer");
        assert!(got.starts_with("seed"), "unexpected body: {got:?}");
    }

    /// TOCTOU: while a symlink flips between two fixed files, every
    /// accepted resolution must bind the canonical NAME to the bytes that
    /// name owns. A single mismatch is a live check-vs-use split.
    #[cfg(unix)]
    #[test]
    fn refut_identity_binding_survives_a_live_symlink_flip() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path().to_path_buf();
        fs::write(root.join("a.txt"), "AAA").expect("a");
        fs::write(root.join("b.txt"), "BBB").expect("b");
        let link = root.join("link");
        std::os::unix::fs::symlink(root.join("a.txt"), &link).expect("symlink");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        let r2 = root.clone();
        let flipper = std::thread::spawn(move || {
            let link = r2.join("link");
            let tmpl = r2.join(".link.tmp");
            let mut on_a = true;
            while !s2.load(std::sync::atomic::Ordering::Relaxed) {
                let target = if on_a {
                    r2.join("b.txt")
                } else {
                    r2.join("a.txt")
                };
                let _ = fs::remove_file(&tmpl);
                if std::os::unix::fs::symlink(&target, &tmpl).is_ok() {
                    let _ = fs::rename(&tmpl, &link);
                }
                on_a = !on_a;
            }
        });

        let mut ok = 0usize;
        let mut refused = 0usize;
        let mut violations: Vec<String> = Vec::new();
        for _ in 0..20_000 {
            match resolve_target(&link) {
                Ok(t) => {
                    let name = t
                        .canonical()
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?")
                        .to_string();
                    let body = t.read_to_string().unwrap_or_default();
                    let want = match name.as_str() {
                        "a.txt" => "AAA",
                        "b.txt" => "BBB",
                        _ => "?",
                    };
                    if body != want {
                        violations.push(format!("canonical={name} body={body:?}"));
                    }
                    ok += 1;
                }
                Err(_) => refused += 1,
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        flipper.join().expect("flipper");
        eprintln!(
            "REFUT toctou: ok={ok} refused={refused} violations={}",
            violations.len()
        );
        assert!(ok > 0, "PRECONDITION: no resolution ever succeeded");
        assert!(
            violations.is_empty(),
            "REFUTED: canonical name bound to foreign bytes: {violations:?}"
        );
    }
}
