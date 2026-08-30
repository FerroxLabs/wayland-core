//! Every override of a [`DISCLOSURE_METHODS`] method in this crate must be
//! registered in [`BACKENDS_THAT_DISCLOSE`], against the method it overrides,
//! so that the read-grading in `wcore_cli::sandbox_cmd::disclosure_tests` is
//! TOTAL over the disclosure surface rather than over the parts somebody
//! remembered.
//!
//! # The defect this exists to make impossible
//!
//! `#368` c6 was graded `met` on `every_declared_limitation_names_its_tracker_
//! and_says_it_is_not_fixed`, which asserts two `const &str` are well formed.
//! A verifier replaced `AppContainerBackend::known_limitations`'s body with
//! `Vec::new()`. It compiled; `wayland-core sandbox status --json` on real
//! Windows returned `"known_limitations":[]`, so the disclosure was gone from
//! the only surface an operator reads; and all 249 tests in this crate stayed
//! green, because nothing anywhere called that method. The parallel
//! `WindowsJobObjectBackend` override was asserted, so the hole was exactly one
//! backend wide.
//!
//! # The N+1, measured rather than imagined
//!
//! Closing that for `known_limitations` alone left the SAME hole open one
//! method over, and it was still open when this file was first written.
//! Replacing `SandboxRegistry::unavailable_reason`'s delegate with `None`
//! compiles (`CHECK_EXIT=0`) and leaves the whole `wcore-sandbox` +
//! `wcore-cli` suite green — 3927 tests, `RED1_TESTS_EXIT=0` — while the
//! identical mutation on the sibling `known_limitations` delegate reddens
//! `every_declaring_backends_disclosure_reaches_both_operator_arms`. Same
//! file, adjacent functions, opposite outcomes: a coverage hole, not a broken
//! instrument. `unavailable_reason` is the field `#369` c2 turns on — the read
//! that replaces twelve silent days — and every assertion on it built a
//! `SandboxStatus` BY HAND, so no test ever traversed the path an operator
//! does.
//!
//! So the scan is total over the method SET, not over one method: a disclosure
//! method added later (`#400` c1 adds `blocks_powershell`) is registered the
//! same way, and an override of it that nothing reads reddens here first.

use std::path::{Path, PathBuf};
use wcore_sandbox::backends::{BACKENDS_THAT_DISCLOSE, DISCLOSURE_METHODS};

/// Not an override: the `SandboxBackend` trait defaults live here.
const TRAIT_DEFAULT: &str = "backends/mod.rs";
/// Not an override: `SandboxRegistry`'s delegates live here.
const REGISTRY_DELEGATE: &str = "lib.rs";

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `src`-relative, with `/` separators on every platform so the comparison
/// against the table is not a Windows-only failure.
fn relative(src: &Path, file: &Path) -> String {
    file.strip_prefix(src)
        .expect("walked from src")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `(source_file, method)` pairs, sorted, so a set comparison names exactly
/// which override is unregistered rather than only that the sets differ.
fn sorted_pairs(mut pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    pairs.sort();
    pairs
}

#[test]
fn every_disclosure_override_is_registered_for_read_grading() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);

    // POSITIVE CONTROL on the scanner. An empty or half-read walk would make
    // the set comparison below pass vacuously, which is the exact failure
    // shape this file exists to close.
    assert!(
        files.len() > 20,
        "the scanner found only {} files under {src:?} — it is not reading the crate",
        files.len()
    );
    // POSITIVE CONTROL on the surface. An empty method list would scan for
    // nothing and compare two empty sets.
    assert!(
        DISCLOSURE_METHODS.len() >= 2,
        "the disclosure surface is {DISCLOSURE_METHODS:?}; a surface this \
         small means the constant was emptied, and every check over it is then \
         vacuous"
    );

    let texts: Vec<(String, String)> = files
        .iter()
        .filter_map(|f| {
            std::fs::read_to_string(f)
                .ok()
                .map(|t| (relative(&src, f), t))
        })
        .collect();

    let mut found: Vec<(String, String)> = Vec::new();
    for method in DISCLOSURE_METHODS {
        let marker = format!("fn {method}(");
        let mut excluded_seen: Vec<String> = Vec::new();
        for (rel, text) in &texts {
            if !text.contains(&marker) {
                continue;
            }
            if rel == TRAIT_DEFAULT || rel == REGISTRY_DELEGATE {
                excluded_seen.push(rel.clone());
            } else {
                found.push((rel.clone(), (*method).to_owned()));
            }
        }

        // POSITIVE CONTROL on the exclusions, PER METHOD. Every disclosure
        // method has exactly two non-override declaration sites — the trait
        // default and the registry delegate. If either stopped matching, the
        // exclusion would become a silent no-op that could hide a real
        // override behind the same path; if a method were dropped from the
        // trait or the registry, its whole read path would be gone and the
        // scan would still find nothing to complain about.
        excluded_seen.sort();
        let mut expected = vec![REGISTRY_DELEGATE.to_owned(), TRAIT_DEFAULT.to_owned()];
        expected.sort();
        assert_eq!(
            excluded_seen, expected,
            "`{method}` must be declared in BOTH the trait default \
             ({TRAIT_DEFAULT}) and the registry delegate ({REGISTRY_DELEGATE}): \
             a disclosure method the registry does not delegate cannot reach an \
             operator at all, and one the trait does not declare is not part of \
             the surface. If a site moved, update TRAIT_DEFAULT / \
             REGISTRY_DELEGATE rather than leaving an exclusion that matches \
             nothing"
        );
    }

    let registered: Vec<(String, String)> = BACKENDS_THAT_DISCLOSE
        .iter()
        .flat_map(|b| {
            b.declares
                .iter()
                .map(|m| (b.source_file.to_owned(), (*m).to_owned()))
        })
        .collect();

    assert_eq!(
        sorted_pairs(found),
        sorted_pairs(registered),
        "a backend that overrides a disclosure method and is not registered \
         for it in `BACKENDS_THAT_DISCLOSE` is a disclosure nothing grades: \
         #368 c6 was already graded `met` while exactly that hole let \
         `AppContainerBackend::known_limitations` be deleted with every test \
         green, and the same hole was then measured still open on \
         `unavailable_reason`. Add the (file, method) pair here AND an arm in \
         `wcore_cli::sandbox_cmd::disclosure_tests`."
    );
}

/// Every method a row claims to declare must be part of the surface, or a row
/// could satisfy the comparison above with a name nothing else knows about.
#[test]
fn every_registered_row_names_a_real_file_a_backend_and_a_real_method() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        !BACKENDS_THAT_DISCLOSE.is_empty(),
        "an empty table makes every check over it vacuous"
    );
    for row in BACKENDS_THAT_DISCLOSE {
        let mut path = src.clone();
        for part in row.source_file.split('/') {
            path.push(part);
        }
        assert!(
            path.is_file(),
            "row `{}` names {path:?}, which is not a file",
            row.name
        );
        assert!(!row.name.is_empty(), "a row must name its backend");
        assert!(
            !row.declares.is_empty(),
            "row `{}` declares nothing, so it is graded by nothing",
            row.name
        );
        for method in row.declares {
            assert!(
                DISCLOSURE_METHODS.contains(method),
                "row `{}` claims to declare `{method}`, which is not in the \
                 disclosure surface {DISCLOSURE_METHODS:?}",
                row.name
            );
        }
    }
}
