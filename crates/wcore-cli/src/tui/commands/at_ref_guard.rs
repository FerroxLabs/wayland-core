//! Guardrails for `@`-reference resolution: the secret denylist and the
//! `.gitignore` matcher.
//!
//! Both guardrails answer one question — *may this path be attached to a
//! message?* — and both err toward exclusion when uncertain, because the
//! cost of leaking a secret or an ignored artifact outweighs the cost of
//! a missed attachment the user can re-request explicitly. Split out of
//! `at_refs.rs` (W3-B) so parsing, completion, and resolution each import
//! only the guard surface they need.

use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

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
// Identity-bound opening
// ─────────────────────────────────────────────────────────────────────────

/// Why [`open_attached`] refused to hand back a readable file.
#[derive(Debug)]
pub enum OpenRefusal {
    /// The path does not exist, or does not resolve to a regular file.
    NotFound,
    /// The name resolved to one object while the guard's view of it named
    /// another — the link moved mid-check. Refused, never retried.
    Raced,
    /// The filesystem said no.
    Io(String),
}

/// A regular file opened ONCE, together with the canonical path proven to
/// name the very object the handle holds.
///
/// The reason this is a type and not a `(PathBuf, String)` helper: the
/// guard needs a NAME to match the denylists against, and the read needs
/// BYTES. Taking the name from one filesystem lookup and the bytes from a
/// second — even via the same path string — IS the defect (core#339): a
/// symlink can be repointed in between. Here the bytes come from `handle`,
/// the name is `target`, and [`open_attached`] has already proven that the
/// two describe one object.
pub struct AttachTarget {
    handle: Handle,
    target: PathBuf,
}

impl AttachTarget {
    /// The canonical, symlink-free path of the object this handle holds.
    /// This — not the name the user typed — is what the denylists see.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Read the OPENED HANDLE.
    ///
    /// Deliberately consumes `self`, and deliberately exposes no way to get
    /// at the path for a second open: re-opening `target` after the guard
    /// has passed is precisely the race this type exists to prevent, so the
    /// API does not offer it.
    pub fn read_to_string(mut self) -> Result<String, OpenRefusal> {
        let mut buf = String::new();
        self.handle
            .as_file_mut()
            .read_to_string(&mut buf)
            .map_err(|e| OpenRefusal::Io(e.to_string()))?;
        Ok(buf)
    }
}

// Test seam fired between the open and the canonicalize — precisely the
// window `open_attached`'s identity comparison exists to close. It is here so
// that race can be exercised DETERMINISTICALLY rather than argued about in a
// comment; outside the crate's own test build it compiles to an empty
// function. (A `///` comment cannot sit on a macro invocation.)
#[cfg(test)]
thread_local! {
    static RACE_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn fire_race_hook() {
    // Taken, not borrowed: it fires once, and cannot re-enter itself.
    let hook = RACE_HOOK.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(not(test))]
fn fire_race_hook() {}

/// Open `named` once and bind the canonical path that names it to the
/// opened handle, so the caller can guard a name and read bytes knowing
/// both describe the same object.
///
/// The order is the whole fix:
///
/// 1. `File::open` pins the object. From here the bytes are settled —
///    repointing the link afterwards changes nothing this handle reads.
/// 2. `canonicalize` produces the name the denylists will match on.
/// 3. The two are compared by FILESYSTEM IDENTITY — device + inode on
///    Unix, volume serial + file index on Windows, both via `same_file`,
///    so no `cfg` branch appears here. On disagreement the link moved
///    between steps 1 and 2 and the request is REFUSED rather than
///    retried; a retry loop is a race the attacker simply re-enters.
///
/// The obvious-looking alternative — canonicalize, guard, then re-open the
/// canonical path — is not equivalent: the final component of a canonical
/// path can still be swapped for a symlink after the guard has passed.
///
/// NOT covered, and not coverable by any name-based guard: a HARD link.
/// Two names for one inode are the same object by construction, so
/// identity cannot separate them — only the name can, and the attacker
/// chose it. That is unchanged here and holds equally for every
/// name-matching denylist in the codebase.
pub fn open_attached(named: &Path) -> Result<AttachTarget, OpenRefusal> {
    let file = File::open(named).map_err(to_refusal)?;
    if !file
        .metadata()
        .map_err(|e| OpenRefusal::Io(e.to_string()))?
        .is_file()
    {
        // A directory opens successfully on Unix. `@file` answered "not
        // found" for one before this function existed, and still must.
        return Err(OpenRefusal::NotFound);
    }
    let handle = Handle::from_file(file).map_err(|e| OpenRefusal::Io(e.to_string()))?;
    fire_race_hook();
    let target = fs::canonicalize(named).map_err(to_refusal)?;
    if handle != Handle::from_path(&target).map_err(to_refusal)? {
        return Err(OpenRefusal::Raced);
    }
    Ok(AttachTarget { handle, target })
}

/// The canonical, symlink-free form of a directory the `@dir` walk is about
/// to descend, or of the workspace root itself.
///
/// Falls back to the path as given when it cannot be resolved, which
/// reproduces exactly the comparison this code did before — never worse.
pub fn canonical_dir(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn to_refusal(e: std::io::Error) -> OpenRefusal {
    if e.kind() == std::io::ErrorKind::NotFound {
        OpenRefusal::NotFound
    } else {
        OpenRefusal::Io(e.to_string())
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
    use tempfile::TempDir;

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

    // ── identity is bound to the HANDLE, not re-derived from the path ────

    /// The trap core#339 names explicitly: canonicalize-then-reopen-by-path
    /// reintroduces the race. This pins the difference deterministically —
    /// no threads, no timing.
    ///
    /// The canonical target's FINAL COMPONENT is replaced between the guard
    /// and the read. An implementation that re-opened `target()` would
    /// return the substituted bytes; one that reads the handle it already
    /// holds returns the bytes it was guarded on. Repointing the *link*
    /// would not discriminate — both implementations resolve `link` only
    /// once — so the swap has to happen at the resolved name.
    #[cfg(unix)]
    #[test]
    fn the_read_returns_the_object_that_was_guarded_not_the_name_it_had() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let real = root.join("real.txt");
        fs::write(&real, "GUARDED-OBJECT").expect("write");
        std::os::unix::fs::symlink(&real, root.join("link.txt")).expect("symlink");

        let opened = open_attached(&root.join("link.txt")).expect("open");
        assert_eq!(opened.target(), real.canonicalize().expect("canon"));

        // The window an attacker owns: same name, different object.
        fs::remove_file(&real).expect("unlink");
        fs::write(&real, "SUBSTITUTED-AFTER-THE-GUARD").expect("rewrite");

        assert_eq!(
            opened.read_to_string().expect("read"),
            "GUARDED-OBJECT",
            "the read followed the path again instead of the opened handle"
        );
    }

    /// `@file` on a directory answered `NotFound` before `open_attached`
    /// existed, and must still — `File::open` succeeds on a directory on
    /// Unix, so this is a real way the refactor could have changed
    /// behaviour.
    #[test]
    fn opening_a_directory_as_a_file_is_not_found() {
        let tmp = TempDir::new().expect("tempdir");
        fs::create_dir(tmp.path().join("sub")).expect("mkdir");
        assert!(matches!(
            open_attached(&tmp.path().join("sub")),
            Err(OpenRefusal::NotFound)
        ));
        // Control: a regular file in the same directory opens fine, so the
        // assertion above cannot pass by refusing everything.
        fs::write(tmp.path().join("f.txt"), "x").expect("write");
        assert!(open_attached(&tmp.path().join("f.txt")).is_ok());
    }

    /// The EXPLOITABLE direction of the race, and the only reason the
    /// identity comparison in [`open_attached`] earns its place.
    ///
    /// The open lands on the credential store; the link is then repointed at
    /// a harmless file. A guard that re-derives the name from the path would
    /// clear `benign.txt` while the handle still holds the secret — and the
    /// read, correctly bound to that handle, would return the secret. Only
    /// comparing the two by filesystem identity catches it.
    ///
    /// The mirror direction (open lands on the benign file, link repointed
    /// at the secret) is safe with or without the comparison: the guard then
    /// sees the secret's name and refuses. So this is the case that matters.
    #[cfg(unix)]
    #[test]
    fn a_link_repointed_after_the_open_is_refused_not_read() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let secret = root.join(".git-credentials");
        fs::write(&secret, "https://fake-user:fake-token@example.invalid\n").expect("write");
        let benign = root.join("benign.txt");
        fs::write(&benign, "harmless").expect("write");
        let link = root.join("notes.txt");
        std::os::unix::fs::symlink(&secret, &link).expect("symlink");

        // Control: with no race, this link opens and reports the secret as
        // its target — so the refusal below cannot be an artifact of the
        // fixture failing to resolve.
        let clean = open_attached(&link).expect("control open");
        assert_eq!(clean.target(), secret.canonicalize().expect("canon"));

        let (l, b) = (link.clone(), benign.clone());
        RACE_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                fs::remove_file(&l).expect("unlink");
                std::os::unix::fs::symlink(&b, &l).expect("repoint");
            }))
        });

        let got = open_attached(&link);
        assert!(
            matches!(got, Err(OpenRefusal::Raced)),
            "a link repointed mid-check was accepted, target={:?}",
            got.map(|o| o.target().to_path_buf())
        );
    }
}
