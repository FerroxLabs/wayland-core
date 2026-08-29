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
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────
// Secret denylist
// ─────────────────────────────────────────────────────────────────────────

/// True if `path` is on the secret denylist. UX doc §3b: `@` "respects the
/// gitignore + a denylist (`.env`, key files) — never silently attach a
/// secret."
///
/// ONE list, ONE owner (core#323): every rule lives in
/// [`wcore_tools::workspace_policy::is_secret_path_static`], the predicate
/// `Read`, `Grep`, `SecretDenyFs` and the Bash deny walk already enforce. This
/// module used to keep a parallel copy; the two drifted in BOTH directions, and
/// #323's first cut only taught the `@` surface to consult both — which closed
/// the `@` half and left eleven credential names readable by the MODEL. Two
/// lists that must agree drift again, so the file-name rules moved into the
/// shared list and this function contributes the one thing the `@` surface
/// genuinely needs: a leading separator.
///
/// The shared rules match separator-anchored PATH FRAGMENTS (`/.ssh/`,
/// `/.git-credentials`), so a bare relative path — exactly what a user types
/// after `@` — misses every one of them. Anchoring uses a synthetic root rather
/// than the process CWD: it only ever adds the separator the fragment rules
/// need, and cannot import an ambient directory that would deny an unrelated
/// file.
///
/// Purely lexical, deliberately — this runs inside the completion loop, on
/// paths that need not exist. That makes it a FLOOR, not the decision: a
/// lexical name and the bytes behind it are not the same file when a symlink is
/// in the way, so the authoritative check runs against the RESOLVED path in
/// [`super::at_ref_resolve`] (core#339).
pub fn is_secret_path(path: &Path) -> bool {
    let anchored;
    let for_fragments = if path.is_absolute() {
        path
    } else {
        anchored = Path::new(std::path::MAIN_SEPARATOR_STR).join(path);
        anchored.as_path()
    };
    wcore_tools::workspace_policy::is_secret_path_static(for_fragments)
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
    /// workspace-policy denylist DIVERGED while there were two of them.
    ///
    /// Nineteen were carried only by
    /// `workspace_policy::is_secret_path_static` — the list `Read`, `Grep` and
    /// `SecretDenyFs` enforce — and were invisible to this module's file-name
    /// rules. Eleven were carried only by this module. Neither list was a
    /// superset of the other, which is how a `@`-attach could inline a file
    /// `Read` refused and vice versa.
    ///
    /// core#323 closed that by DELETING this module's list rather than syncing
    /// it: the rules now live in one place with one owner. The table stays as a
    /// regression pin — it goes red if the merged list loses any entry either
    /// half used to carry, which is the failure the drift produced.
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
}
