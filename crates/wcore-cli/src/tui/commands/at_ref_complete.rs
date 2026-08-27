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

use super::at_ref_guard::{GitIgnore, canonical_root, is_secret_path, rel_to_root};

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
    let croot = canonical_root(root);
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
        // Lexical pass, on the name as listed.
        if is_secret_path(&path) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let rel = if dir_part.is_empty() {
            name.to_string()
        } else {
            format!("{dir_part}/{name}")
        };
        // Identity pass — the third production call site of the guard.
        // Offering a row is the first half of attaching it: the user presses
        // Tab and `resolve_file` inlines whatever the name points at, so the
        // popup has to judge the same object that read will (core#339).
        //
        // `canonicalize` rather than `resolve_target` because nothing is read
        // here, so there is no handle to bind a name to; the authoritative,
        // race-free guard is the one at resolution. A candidate whose target
        // will not resolve keeps its lexical verdict — a broken link leaks
        // nothing and still deserves to be listed.
        let canonical = fs::canonicalize(&path).ok();
        if let Some(canonical) = &canonical
            && is_secret_path(canonical)
        {
            continue;
        }
        // Judge the gitignore at the target's real location too, so the
        // popup and the resolver cannot disagree about the same candidate.
        // A target outside the root has no gitignore jurisdiction, and falls
        // back to the path as typed.
        let rel_for_ignore = canonical
            .as_deref()
            .and_then(|c| rel_to_root(c, &croot))
            .unwrap_or_else(|| rel.clone());
        if ignore.is_ignored(&rel_for_ignore, is_dir) {
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

    /// The third production call site of the secret guard. The popup listing
    /// a credential store is the whole leak: the user presses Tab and the
    /// content is inlined.
    #[test]
    fn completion_never_offers_a_workspace_policy_secret() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(
            root.join(".git-credentials"),
            "https://fake-user:fake-token@example.invalid\n",
        )
        .expect("write fixture");
        fs::write(root.join(".gitignore"), "").expect("write gitignore");

        let comps = complete("@.git", root);
        let inserts: Vec<&str> = comps.iter().map(|c| c.insert.as_str()).collect();
        assert!(
            !inserts.iter().any(|i| i.contains(".git-credentials")),
            "completion offered a credential store: {inserts:?}"
        );
        // Control: an ordinary dotfile in the same directory IS offered, so
        // the refutation above cannot pass by listing nothing.
        assert!(inserts.contains(&"@.gitignore"), "{inserts:?}");
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

    /// core#339, production call site 3 of 3 — the completion popup.
    ///
    /// Offering the candidate is the first half of attaching it: the user
    /// presses Tab and `resolve_file` inlines whatever the name points at.
    /// The popup therefore has to judge the same thing the read will.
    #[cfg(unix)]
    #[test]
    fn completion_never_offers_a_symlink_to_a_credential_store() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("ws");
        let outside = tmp.path().join("home");
        fs::create_dir_all(&root).expect("mkdir ws");
        fs::create_dir_all(&outside).expect("mkdir home");
        fs::write(
            outside.join(".git-credentials"),
            "https://fake-user:fake-token@example.invalid\n",
        )
        .expect("write fixture");
        std::os::unix::fs::symlink(outside.join(".git-credentials"), root.join("notes.txt"))
            .expect("symlink");
        fs::write(root.join("notepad.txt"), "ordinary").expect("write control");

        let inserts: Vec<String> = complete("@note", &root)
            .into_iter()
            .map(|c| c.insert)
            .collect();
        // Control: the ordinary sibling IS offered, so the refutation below
        // cannot pass by offering nothing.
        assert!(
            inserts.iter().any(|i| i == "@notepad.txt"),
            "control candidate missing: {inserts:?}"
        );
        assert!(
            !inserts.iter().any(|i| i == "@notes.txt"),
            "the popup offered a symlink to a credential store: {inserts:?}"
        );
    }

    /// A symlink to an ordinary file must stay offerable — the popup is not
    /// allowed to hide legitimate symlinked sources.
    #[cfg(unix)]
    #[test]
    fn completion_still_offers_a_symlink_to_an_ordinary_file() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("ws");
        let outside = tmp.path().join("home");
        fs::create_dir_all(&root).expect("mkdir ws");
        fs::create_dir_all(&outside).expect("mkdir home");
        fs::write(outside.join("shared.md"), "shared body").expect("write");
        std::os::unix::fs::symlink(outside.join("shared.md"), root.join("link.md"))
            .expect("symlink");

        let inserts: Vec<String> = complete("@link", &root)
            .into_iter()
            .map(|c| c.insert)
            .collect();
        assert!(inserts.iter().any(|i| i == "@link.md"), "{inserts:?}");
    }

    /// The popup's gitignore verdict must be ADDITIVE, not substitutive.
    ///
    /// A candidate has two relative names — the one it is typed as and the
    /// one its target canonicalizes to — and either being ignored is a
    /// reason not to offer it. Judging only the canonical name un-ignores
    /// every lexically-ignored link: the popup offers the row, the user
    /// presses Tab, and `resolve_file`'s own lexical check refuses it —
    /// which is exactly the popup/resolver disagreement the identity pass
    /// was added to remove. Both directions need a symlink to be visible at
    /// all: a real file's two names are equal, so the passes cannot differ.
    #[cfg(unix)]
    #[test]
    fn completion_honors_both_the_typed_and_the_canonical_gitignore_name() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "notes.txt\nbuild/\n").expect("write gitignore");
        fs::create_dir(root.join("real")).expect("mkdir real");
        fs::create_dir(root.join("build")).expect("mkdir build");
        fs::write(root.join("real/out.txt"), "kept body").expect("write out");
        fs::write(root.join("build/out.txt"), "ignored body").expect("write build out");
        // Ignored by the name it is TYPED as; its target is not ignored.
        std::os::unix::fs::symlink(root.join("real/out.txt"), root.join("notes.txt"))
            .expect("symlink notes");
        // Ignored by the name it CANONICALIZES to; its own name is not.
        std::os::unix::fs::symlink(root.join("build/out.txt"), root.join("notable.txt"))
            .expect("symlink notable");
        fs::write(root.join("noted.txt"), "ordinary").expect("write control");

        let inserts: Vec<String> = complete("@no", root)
            .into_iter()
            .map(|c| c.insert)
            .collect();
        // Control: the ordinary sibling IS offered, so neither refutation
        // below can pass by offering nothing.
        assert!(
            inserts.iter().any(|i| i == "@noted.txt"),
            "control candidate missing: {inserts:?}"
        );
        assert!(
            !inserts.iter().any(|i| i == "@notes.txt"),
            "offered a candidate the resolver refuses as git-ignored: {inserts:?}"
        );
        assert!(
            !inserts.iter().any(|i| i == "@notable.txt"),
            "offered a link whose target is git-ignored: {inserts:?}"
        );
    }
}
