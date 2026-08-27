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
/// regular file, so the later `read_to_string` is unchanged. Windows has
/// no equivalent flag and no equivalent exposure through a workspace path
/// (a named pipe there is a `\\.\pipe\…` name, not a directory entry),
/// so it keeps the plain open.
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
}
