//! Pre-flight hint for the Desktop contract corpus.
//!
//! A change that edits a `SOURCE_INPUTS` file without re-pinning
//! `contracts/desktop/v1` will drift the corpus: `source_inputs_digest` moves,
//! `fixture_digest` follows, and the corpus check fails on a hash rather than
//! on anything about the wire. That is knowable from the changed-file list
//! alone, before a single test runs, which is what this module reports.
//!
//! It is advisory by construction - it returns a message, never an error - so
//! the CI step that carries it can be non-gating.

use std::collections::BTreeSet;

use super::spec::SOURCE_INPUTS;

/// Repo-relative directory holding the checked-in corpus.
///
/// A change that touches anything under here is already re-pinning the corpus,
/// so there is nothing to warn about. Kept in sync with the real corpus root by
/// `the_contract_dir_is_where_the_corpus_actually_lives` below - if these ever
/// part company the re-pin arm of this check goes silently dead and every
/// re-pinning PR gets a false warning.
pub const CONTRACT_DIR: &str = "crates/wcore-protocol/contracts/desktop/v1/";

/// Normalize one changed-file line: trim it and put it in the repo's own
/// forward-slash spelling, so a Windows-produced file list is understood.
fn normalize(path: &str) -> String {
    path.trim().replace('\\', "/")
}

/// The advisory notice for a changed-file list, or `None` when the change
/// cannot drift the corpus (it touched no source input) or has already dealt
/// with it (it re-pins the corpus in the same change).
pub fn preflight_notice(changed_paths: &[String]) -> Option<String> {
    let mut touched = BTreeSet::new();
    let mut repinned = false;
    for raw in changed_paths {
        let path = normalize(raw);
        if path.is_empty() {
            continue;
        }
        if path.starts_with(CONTRACT_DIR) {
            repinned = true;
        }
        if SOURCE_INPUTS.contains(&path.as_str()) {
            touched.insert(path);
        }
    }
    if repinned || touched.is_empty() {
        return None;
    }
    let listed = touched.into_iter().collect::<Vec<_>>().join(", ");
    Some(format!(
        "this change edits Desktop contract SOURCE_INPUTS ({listed}) without touching \
         {CONTRACT_DIR}, so the corpus check will fail on source_inputs_digest - a source-hash \
         rebase, not a protocol break. Safe procedure: run `cargo run -p wcore-protocol --bin \
         wcore-contract -- diff`, confirm only source_inputs_digest and fixture_digest changed, \
         then re-run it with `generate`."
    ))
}

#[cfg(test)]
mod tests {
    use super::super::generate::contract_path;
    use super::*;

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn a_source_input_edit_without_a_repin_is_flagged_with_the_safe_procedure() {
        let notice = preflight_notice(&paths(&[
            "crates/wcore-agent/src/engine.rs",
            "docs/providers.md",
        ]))
        .expect("editing a source input without a re-pin must warn");
        assert!(
            notice.contains("crates/wcore-agent/src/engine.rs"),
            "the notice must name the file that will move the hash: {notice}"
        );
        assert!(
            notice.contains("source_inputs_digest") && notice.contains("wcore-contract -- diff"),
            "the notice must point at the safe procedure, not a blind regenerate: {notice}"
        );
        assert!(
            !notice.contains("docs/providers.md"),
            "only source inputs belong in the notice: {notice}"
        );
    }

    #[test]
    fn a_change_that_repins_the_corpus_is_silent() {
        assert_eq!(
            preflight_notice(&paths(&[
                "crates/wcore-agent/src/engine.rs",
                "crates/wcore-protocol/contracts/desktop/v1/manifest.json",
            ])),
            None,
            "a change that already re-pins the corpus has nothing to be warned about"
        );
    }

    #[test]
    fn a_change_touching_no_source_input_is_silent() {
        assert_eq!(
            preflight_notice(&paths(&["README.md", "crates/wcore-tools/src/read.rs"])),
            None
        );
    }

    #[test]
    fn a_windows_spelled_file_list_is_understood_on_both_arms() {
        assert!(
            preflight_notice(&paths(&["crates\\wcore-agent\\src\\engine.rs"])).is_some(),
            "a backslash-spelled source input must still be recognised"
        );
        assert_eq!(
            preflight_notice(&paths(&[
                "crates\\wcore-agent\\src\\engine.rs",
                "crates\\wcore-protocol\\contracts\\desktop\\v1\\manifest.json",
            ])),
            None,
            "a backslash-spelled re-pin must still silence the notice"
        );
    }

    #[test]
    fn blank_lines_in_a_piped_file_list_are_ignored() {
        assert_eq!(preflight_notice(&paths(&["", "  ", "\n"])), None);
        assert!(
            preflight_notice(&paths(&["", "  crates/wcore-cli/src/main.rs  "])).is_some(),
            "a padded path must still be recognised"
        );
    }

    /// The re-pin arm is a string prefix over a path the generator owns
    /// elsewhere. If the corpus ever moves, this test fails here rather than
    /// the notice quietly firing on every re-pinning PR forever.
    #[test]
    fn the_contract_dir_is_where_the_corpus_actually_lives() {
        let real = contract_path();
        let real = real.to_string_lossy().replace('\\', "/");
        let expected = CONTRACT_DIR.trim_end_matches('/');
        assert!(
            real.ends_with(expected),
            "CONTRACT_DIR ({expected}) no longer matches the real corpus root ({real})"
        );
    }
}
