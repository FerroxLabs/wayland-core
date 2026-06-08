//! Autocomplete for `@`-references — the popup the composer shows when a
//! `@…` token is being typed.
//!
//! Given a partial `@…` token, [`complete`] lists candidate references the
//! user can insert: the four static keyword kinds plus filesystem entries
//! for `@file`/`@dir`. Filesystem candidates are filtered through the
//! [`at_ref_guard`] guardrails so the popup never even *offers* a secret
//! or a git-ignored path — the guardrail starts at discovery, not just at
//! resolution. Split out of `at_refs.rs` (W3-B).

use std::fs;
use std::path::Path;

use super::at_ref_guard::{GitIgnore, is_secret_path};

/// Max completion candidates returned for one partial token. The popup in
/// the mockup shows a short list; more than this is noise.
const MAX_COMPLETIONS: usize = 12;

/// One candidate row in the `@` autocomplete popup (UX doc §3b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The text inserted into the composer when this row is chosen,
    /// including the leading `@` (e.g. `@crates/wcore-config/`).
    pub insert: String,
    /// The right-hand "surfaced as" descriptor (`file · 28 KB`,
    /// `directory`, `symbol · compat.rs:10`).
    pub label: String,
    /// `true` if the candidate is a directory — the composer draws a
    /// trailing `/` and a folder glyph.
    pub is_dir: bool,
}

/// Produce autocomplete candidates for a partial `@…` token typed in the
/// composer. `partial` includes the leading `@`. `root` is the workspace
/// directory the filesystem-backed kinds (`@file`/`@dir`) walk.
///
/// Filesystem candidates skip git-ignored and secret paths so the popup
/// never even *offers* a `.env` — the guardrail starts at discovery, not
/// just at resolution.
///
/// The static kinds (`@diff`, `@url`, `@session`, `@output`) are offered
/// as keyword completions when the partial is a prefix of the keyword.
pub fn complete(partial: &str, root: &Path) -> Vec<Completion> {
    let Some(body) = partial.strip_prefix('@') else {
        return Vec::new();
    };

    let mut out = Vec::new();

    // Static-keyword completions: `@di` → `@diff`, `@o` → `@output`, …
    for kw in ["diff", "url", "session", "output"] {
        if kw.starts_with(body) && kw != body {
            out.push(Completion {
                insert: format!("@{kw}"),
                label: format!("{kw} · static reference"),
                is_dir: false,
            });
        }
    }

    // Filesystem completions for `@file`/`@dir`. The partial is split into
    // a parent directory (already typed) and a leaf prefix to match.
    out.extend(complete_paths(body, root));

    out.truncate(MAX_COMPLETIONS);
    out
}

/// Filesystem-backed completion: list entries of the directory implied by
/// `body`, keeping those whose name starts with the typed leaf.
fn complete_paths(body: &str, root: &Path) -> Vec<Completion> {
    // Split `crates/wcore-co` into dir=`crates/` leaf=`wcore-co`.
    let (dir_part, leaf) = match body.rsplit_once('/') {
        Some((d, l)) => (d, l),
        None => ("", body),
    };

    let scan_dir = if dir_part.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir_part)
    };

    let entries = match fs::read_dir(&scan_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let ignore = GitIgnore::load(root);
    let mut out = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(leaf) {
            continue;
        }
        // Hidden files only surface when the user explicitly typed a `.`
        // — keeps `@`-on-empty from spraying `.git`, `.DS_Store`, etc.
        if name.starts_with('.') && !leaf.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if is_secret_path(&path) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let rel = if dir_part.is_empty() {
            name.to_string()
        } else {
            format!("{dir_part}/{name}")
        };
        if ignore.is_ignored(&rel, is_dir) {
            continue;
        }

        let insert = if is_dir {
            format!("@{rel}/")
        } else {
            format!("@{rel}")
        };
        let label = if is_dir {
            "directory".to_string()
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            format!("file · {}", human_size(size))
        };
        out.push(Completion {
            insert,
            label,
            is_dir,
        });
    }

    // Directories first, then alphabetical — folders are the navigational
    // affordance, files the terminal pick.
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.insert.cmp(&b.insert))
    });
    out
}

/// Human-readable byte size (`28 KB`, `1.4 MB`) for completion labels.
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn completion_offers_static_keywords_on_prefix() {
        let tmp = TempDir::new().expect("tempdir");
        let comps = complete("@di", tmp.path());
        assert!(comps.iter().any(|c| c.insert == "@diff"));
        // `@d` should not yet narrow to a single keyword — but it includes diff.
        let d = complete("@d", tmp.path());
        assert!(d.iter().any(|c| c.insert == "@diff"));
    }

    #[test]
    fn completion_lists_filesystem_entries_matching_the_leaf() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir(root.join("crates")).expect("mkdir crates");
        fs::write(root.join("Cargo.toml"), "[package]").expect("write toml");
        fs::write(root.join("README.md"), "# readme").expect("write readme");

        // Leaf matching is a case-sensitive prefix: `@C` matches `Cargo.toml`
        // but not `crates` (lowercase). `@cr` matches the directory.
        let upper = complete("@C", root);
        assert!(upper.iter().any(|c| c.insert == "@Cargo.toml"));
        assert!(!upper.iter().any(|c| c.insert == "@crates/"));

        let lower = complete("@cr", root);
        let crates = lower.iter().find(|c| c.insert == "@crates/");
        assert!(crates.is_some(), "directory offered with trailing slash");
        // A directory is flagged as such and labelled `directory`.
        let crates = crates.expect("crates dir");
        assert!(crates.is_dir);
        assert_eq!(crates.label, "directory");
    }

    #[test]
    fn completion_descends_into_a_typed_directory() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join("src/tui")).expect("mkdir");
        fs::write(root.join("src/main.rs"), "fn main(){}").expect("write");
        fs::write(root.join("src/lib.rs"), "// lib").expect("write");

        let comps = complete("@src/m", root);
        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].insert, "@src/main.rs");
    }

    #[test]
    fn completion_never_offers_secret_or_gitignored_paths() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "ignored.txt\n").expect("write gi");
        fs::write(root.join("visible.txt"), "ok").expect("write visible");
        fs::write(root.join("ignored.txt"), "no").expect("write ignored");
        fs::write(root.join(".env"), "SECRET=1").expect("write env");

        let comps = complete("@", root);
        let inserts: Vec<_> = comps.iter().map(|c| c.insert.as_str()).collect();
        assert!(inserts.contains(&"@visible.txt"));
        assert!(!inserts.iter().any(|i| i.contains("ignored.txt")));
        assert!(!inserts.iter().any(|i| i.contains(".env")));
    }

    #[test]
    fn completion_caps_the_candidate_count() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        for i in 0..50 {
            fs::write(root.join(format!("file{i:02}.txt")), "x").expect("write");
        }
        let comps = complete("@file", root);
        assert!(comps.len() <= MAX_COMPLETIONS);
    }

    #[test]
    fn completion_requires_a_leading_at() {
        assert!(complete("nope", Path::new(".")).is_empty());
    }

    #[test]
    fn human_size_formats_b_kb_mb() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}
