//! SR-04 / SR-05 — the Grep tool's ignore + secret policy.
//!
//! Grep has three interchangeable backends (`rg`, POSIX `grep`, Windows
//! `findstr`) and, until this module existed, no policy of its own: whatever
//! the backend chose to report was handed to the model. Ripgrep skips hidden
//! files and honours `.gitignore`; `grep -rn` and `findstr` do neither. So the
//! product's secret handling silently depended on whether the host happened to
//! have ripgrep installed — measured: `which rg` finds nothing on the Linux
//! build host and `/opt/homebrew/bin/rg` on the Mac, and the `.env` canary
//! `ZORVAX-7731` reached the provider verbatim on Linux while macOS "passed".
//!
//! The policy therefore lives HERE, computed from the filesystem, and is
//! applied by `run_grep` to whichever backend ran. The backends do no
//! filtering at all, so they cannot diverge from each other.
//!
//! ## The rules, and why
//!
//! 1. **Ignore policy.** For a DIRECTORY search the set of reportable files is
//!    enumerated with `ignore::WalkBuilder` under standard filters — the same
//!    crate and the same defaults ripgrep itself uses, so `.gitignore`,
//!    `.ignore`, `.git/` and hidden dotfiles are excluded on every backend
//!    rather than only where ripgrep is installed. Anything the walker did not
//!    admit is withheld.
//! 2. **Explicitly named paths are still searched.** Naming a hidden file
//!    directly searches it (ripgrep's own rule): the ignore policy governs
//!    TRAVERSAL, not an explicit request.
//! 3. **Secret-shaped files are refused even when named explicitly**, because
//!    Grep returns matched line CONTENT. The predicate is the existing
//!    `workspace_policy::is_secret_path_static` — the same list `Read`,
//!    `SecretDenyFs` and the OS sandbox already use — not a second, divergent
//!    one.
//! 4. **A VCS CONTENT store is never traversed** (core#244 c3). `Bash`'s
//!    subprocess is confined by the OS sandbox, which is handed
//!    `vcs_content_stores(root)` as `fs_read_deny`; the subprocess GREP spawns
//!    is not, and `is_secret_path_static` cannot see a store because a loose
//!    object is named after its hash. `ctx.vfs.exists()` refuses a `path`
//!    argument INSIDE a store, but `.git` and `<vendor>/.git` are not
//!    themselves stores — so naming the directory one component up walked
//!    into `.git/objects` and `.git/lfs` in plaintext. The predicate is
//!    `workspace_policy::is_vcs_content_store_static`, the same lexical arm
//!    `WorkspacePolicy::is_vcs_content_store` uses, not a second copy.
//!    `.git/HEAD` and `.git/refs` are NOT stores and stay searchable, matching
//!    the `git rev-parse` carve-out the OS deny list already makes.
//! 5. **Surviving match content is scrubbed** with `wcore_safety::PIIScrubber`.
//!    This is not belt-and-braces: the engine's central `redact_tool_output`
//!    ALREADY runs that scrubber over every tool result, but its
//!    `SECRET_ASSIGNMENT` rule is `(?im)^\s*`-anchored and Grep prefixes each
//!    hit with `path:lineno:`, which pushes the assignment off the start of the
//!    line and makes the rule unmatchable. Scrubbing the content field alone,
//!    with the prefix removed, is what restores it.
//! 6. **Withholding is reported, never silent.** A search whose every hit was
//!    withheld must not render as "No matches found" — "could not show you" and
//!    "there was nothing" are different answers and the model acts on them
//!    differently.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::path_validation::lex_normalize;
use crate::workspace_policy::{is_secret_path_static, is_vcs_content_store_static};

/// Cap on the number of withheld filenames named in the footer. A name is not
/// a secret — the model needs to know WHICH file it cannot see — but an
/// unbounded list would be its own output-size problem.
const MAX_NAMED_WITHHELD: usize = 5;

/// The reportable-file decision for one Grep invocation, computed before any
/// backend runs.
pub(crate) enum GrepScope {
    /// An explicitly named single file. No traversal happened, so the ignore
    /// policy does not apply and every emitted line belongs to this file.
    File(PathBuf),
    /// A directory search. Only files the walker admitted may be reported.
    Dir(HashSet<PathBuf>),
}

/// The outcome of applying the policy to a backend's raw stdout.
pub(crate) struct Filtered {
    /// Match lines that survived, with their `path:lineno:` prefixes intact
    /// and their content scrubbed.
    pub(crate) lines: Vec<String>,
    /// Basenames of secret-shaped files whose matches were withheld.
    pub(crate) secret_files: BTreeSet<String>,
    /// Matches withheld because the ignore policy excluded their file.
    pub(crate) ignored: usize,
    /// Matches withheld because their file lives in a VCS CONTENT store
    /// (core#244 / #322 / #243). Counted separately from `ignored`: an
    /// operator who searched `.git` deserves to know the store was refused,
    /// not that "something was gitignored".
    pub(crate) vcs_store: usize,
    /// Output lines that named no file we could attribute them to. Dropped
    /// rather than forwarded — an unattributable line cannot be policed, and
    /// an unpoliceable line is exactly what this module exists to stop.
    pub(crate) unattributable: usize,
}

impl Filtered {
    pub(crate) fn withheld_anything(&self) -> bool {
        !self.secret_files.is_empty()
            || self.ignored > 0
            || self.vcs_store > 0
            || self.unattributable > 0
    }

    /// One deterministic line explaining what was withheld, or `None` when
    /// nothing was.
    pub(crate) fn footer(&self) -> Option<String> {
        if !self.withheld_anything() {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if !self.secret_files.is_empty() {
            let named: Vec<&str> = self
                .secret_files
                .iter()
                .take(MAX_NAMED_WITHHELD)
                .map(String::as_str)
                .collect();
            let more = self.secret_files.len().saturating_sub(named.len());
            let suffix = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            parts.push(format!(
                "{} secret-shaped file(s) withheld ({}{})",
                self.secret_files.len(),
                named.join(", "),
                suffix
            ));
        }
        if self.ignored > 0 {
            parts.push(format!("{} match(es) in ignored paths", self.ignored));
        }
        if self.vcs_store > 0 {
            parts.push(format!(
                "{} match(es) in VCS content stores withheld",
                self.vcs_store
            ));
        }
        if self.unattributable > 0 {
            parts.push(format!(
                "{} unattributable output line(s)",
                self.unattributable
            ));
        }
        Some(format!("[Grep policy: {}]", parts.join("; ")))
    }
}

/// Decide, from the filesystem, which files this search may report on.
///
/// `resolved` is the already-validated, absolute, lexically normalized search
/// target produced by `validate_search_root`.
pub(crate) fn scope_for(resolved: &Path) -> GrepScope {
    if !resolved.is_dir() {
        return GrepScope::File(lex_normalize(resolved));
    }
    let mut allowed = HashSet::new();
    // `standard_filters(true)` is ripgrep's own default set: .gitignore,
    // .ignore, global gitignore, .git/ exclusion and hidden-file skipping.
    // Using ripgrep's crate rather than re-deriving the rules is what makes
    // "the same policy on all three backends" true by construction.
    //
    // `require_git(false)` is a DELIBERATE, stated divergence from ripgrep's
    // default, and the only one. MEASURED on ripgrep 14: outside a git
    // repository `rg` does not read `.gitignore` at all, so a directory holding
    // `.gitignore` with `secrets/` in it hands `secrets/` straight back the
    // moment it is not (yet) a repo. The file is an explicit statement of
    // intent by the person whose directory this is, and honouring it should not
    // wait on `git init`. This errs toward not leaking, and it is never silent:
    // whatever it withholds is counted in the footer.
    for entry in ignore::WalkBuilder::new(resolved)
        .standard_filters(true)
        .require_git(false)
        .follow_links(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        // Rule 3: a secret-shaped file is never reportable, even though the
        // walker admitted it (`config/prod.pem` is neither hidden nor
        // gitignored).
        //
        // Rule 4: nor is anything inside a VCS content store. The walker
        // admits it whenever the search target is the store's PARENT
        // (`Grep(path = ".git")`), because the `.git/` filter only prunes a
        // `.git` found DURING traversal, and a loose object is neither hidden
        // nor secret-NAMED.
        if is_secret_path_static(path) || is_vcs_content_store_static(path) {
            continue;
        }
        allowed.insert(lex_normalize(path));
    }
    GrepScope::Dir(allowed)
}

impl GrepScope {
    /// True when this scope may report matches from `path`.
    fn admits(&self, path: &Path) -> bool {
        match self {
            // Rule 2 + rule 3: an explicitly named file is searched, unless it
            // is secret-shaped.
            GrepScope::File(f) => {
                path == f && !is_secret_path_static(path) && !is_vcs_content_store_static(path)
            }
            GrepScope::Dir(allowed) => allowed.contains(path),
        }
    }

    /// Apply the policy to one backend's raw stdout.
    ///
    /// `base` is the subprocess working directory, so a relative path in the
    /// backend's output is resolved exactly as the backend meant it.
    pub(crate) fn apply(&self, stdout: &str, base: &Path) -> Filtered {
        let mut out = Filtered {
            lines: Vec::new(),
            secret_files: BTreeSet::new(),
            ignored: 0,
            vcs_store: 0,
            unattributable: 0,
        };

        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((path, content_start)) = self.attribute(line, base) else {
                out.unattributable += 1;
                continue;
            };
            if !self.admits(&path) {
                if is_secret_path_static(&path) {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.to_string_lossy().into_owned());
                    out.secret_files.insert(name);
                } else if is_vcs_content_store_static(&path) {
                    out.vcs_store += 1;
                } else {
                    out.ignored += 1;
                }
                continue;
            }
            // Rule 5: scrub the CONTENT only. The `path:lineno:` prefix is what
            // defeats the line-anchored credential rules, so it must not be
            // part of the string handed to the scrubber.
            let (prefix, content) = line.split_at(content_start);
            let scrubbed = wcore_safety::PIIScrubber.scrub(content);
            out.lines.push(format!("{prefix}{scrubbed}"));
        }

        out
    }

    /// Work out which file a backend output line belongs to, and where that
    /// line's CONTENT starts.
    fn attribute(&self, line: &str, base: &Path) -> Option<(PathBuf, usize)> {
        match self {
            GrepScope::File(f) => {
                // Backends disagree on whether a single-file search echoes the
                // filename: `rg` and `findstr` (with an explicit spec) do,
                // GNU `grep -rn` on one file does not — measured, it emits
                // `1:SECRET=...`. Both shapes attribute to the same file.
                let name = f.to_string_lossy();
                let after_name = match line.strip_prefix(&format!("{name}:")) {
                    Some(rest) => line.len() - rest.len(),
                    None => 0,
                };
                let rest = &line[after_name..];
                let digits = leading_digits(rest);
                let content_start = if digits > 0 && rest.as_bytes().get(digits) == Some(&b':') {
                    after_name + digits + 1
                } else {
                    after_name
                };
                Some((f.clone(), content_start))
            }
            GrepScope::Dir(_) => {
                let (path, content_start) = split_match_line(line)?;
                Some((resolve_against(base, Path::new(path)), content_start))
            }
        }
    }
}

fn leading_digits(s: &str) -> usize {
    s.bytes().take_while(u8::is_ascii_digit).count()
}

/// Split `<path>:<lineno>:<content>`, returning the path and the byte offset at
/// which the content begins.
///
/// Scanning for the FIRST `:<digits>:` (rather than the first colon) is what
/// makes a Windows drive prefix safe: in `C:\proj\x.rs:12:hit` the colon after
/// `C` is followed by a separator, not a digit, so the split lands on `:12:`.
fn split_match_line(line: &str) -> Option<(&str, usize)> {
    let mut from = 0;
    while let Some(rel) = line[from..].find(':') {
        let colon = from + rel;
        let rest = &line[colon + 1..];
        let digits = leading_digits(rest);
        if digits > 0 && rest.as_bytes().get(digits) == Some(&b':') {
            return Some((&line[..colon], colon + 1 + digits + 1));
        }
        from = colon + 1;
    }
    None
}

/// Resolve a backend-emitted path the way the backend meant it: relative to the
/// subprocess working directory, then lexically normalized so `./sub/x.txt` and
/// `<base>/sub/x.txt` are the same key as the walker's entry.
fn resolve_against(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        lex_normalize(path)
    } else {
        lex_normalize(&base.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_match_line_survives_a_windows_drive_prefix() {
        let (path, start) = split_match_line(r"C:\proj\x.rs:12:let x = 1;").expect("split");
        assert_eq!(path, r"C:\proj\x.rs");
        assert_eq!(&r"C:\proj\x.rs:12:let x = 1;"[start..], "let x = 1;");
    }

    #[test]
    fn split_match_line_takes_the_first_line_number_not_a_later_one() {
        let line = "notes.txt:12:foo:34:bar";
        let (path, start) = split_match_line(line).expect("split");
        assert_eq!(path, "notes.txt");
        assert_eq!(&line[start..], "foo:34:bar");
    }

    /// A bare `<digits>:<content>` line (GNU `grep -rn` on a single file) names
    /// no path. In directory scope that must be dropped rather than guessed at.
    #[test]
    fn split_match_line_rejects_a_bare_line_number() {
        assert!(split_match_line("12:SECRET=x").is_none());
    }

    /// Both single-file output shapes must attribute to the same file and
    /// isolate the same content, or the scrubber sees the wrong bytes.
    #[test]
    fn file_scope_attributes_both_single_file_output_shapes() {
        let f = PathBuf::from("/tmp/w/.env");
        let scope = GrepScope::File(f.clone());
        let base = Path::new("/tmp/w");

        let (p1, s1) = scope.attribute("1:SECRET=x", base).expect("bare form");
        assert_eq!(p1, f);
        assert_eq!(&"1:SECRET=x"[s1..], "SECRET=x");

        let named = "/tmp/w/.env:1:SECRET=x";
        let (p2, s2) = scope.attribute(named, base).expect("named form");
        assert_eq!(p2, f);
        assert_eq!(&named[s2..], "SECRET=x");
    }

    /// The footer must never round a withholding down to silence.
    #[test]
    fn footer_names_what_was_withheld_and_is_absent_when_nothing_was() {
        let mut f = Filtered {
            lines: Vec::new(),
            secret_files: BTreeSet::new(),
            ignored: 0,
            vcs_store: 0,
            unattributable: 0,
        };
        assert!(f.footer().is_none());
        f.secret_files.insert(".env".to_string());
        f.ignored = 2;
        let footer = f.footer().expect("footer");
        assert!(footer.contains(".env"), "{footer}");
        assert!(footer.contains("2 match(es) in ignored paths"), "{footer}");
        // core#244 c3: a withheld VCS-store match is reported as such, not
        // folded into "ignored paths" and not rounded down to silence.
        f.vcs_store = 3;
        let footer = f.footer().expect("footer");
        assert!(
            footer.contains("3 match(es) in VCS content stores withheld"),
            "{footer}"
        );
    }
}
