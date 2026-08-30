//! Every `known_limitations` override in this crate must be registered in
//! [`BACKENDS_THAT_DECLARE_LIMITATIONS`], so that the read-grading in
//! `wcore_cli::sandbox_cmd::disclosure_tests` is TOTAL over the backends that
//! declare something rather than over the ones somebody remembered.
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
//! Enumerating the two known backends would close today's instance and leave
//! the shape open. The predicate here is total instead: *every* file under
//! `src/` that overrides `known_limitations` must appear in the table, so a
//! backend added later cannot declare a limitation that no test reads without
//! reddening this file first.

use std::path::{Path, PathBuf};
use wcore_sandbox::backends::BACKENDS_THAT_DECLARE_LIMITATIONS;

/// The signature of an override. The trait's own default and the registry's
/// delegating wrapper match it too, and are excluded BY NAME below, with an
/// assertion that each excluded file really does still contain it — an
/// exclusion that silently stops matching would shrink the scan's input and
/// make the comparison pass for the wrong reason.
const OVERRIDE_MARKER: &str = "fn known_limitations(";

/// Not an override: the `SandboxBackend` trait default lives here.
const TRAIT_DEFAULT: &str = "backends/mod.rs";
/// Not an override: `SandboxRegistry::known_limitations` delegates from here.
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

#[test]
fn every_known_limitations_override_is_registered_for_read_grading() {
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

    let mut found: Vec<String> = Vec::new();
    let mut excluded_seen: Vec<String> = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        if !text.contains(OVERRIDE_MARKER) {
            continue;
        }
        let rel = relative(&src, file);
        if rel == TRAIT_DEFAULT || rel == REGISTRY_DELEGATE {
            excluded_seen.push(rel);
        } else {
            found.push(rel);
        }
    }

    // POSITIVE CONTROL on the exclusions. If a refactor moved the trait
    // default or the registry delegate, the names above would stop matching
    // anything and the exclusion would become a silent no-op that could hide a
    // real override behind the same path.
    excluded_seen.sort();
    let mut excluded_expected = vec![REGISTRY_DELEGATE.to_owned(), TRAIT_DEFAULT.to_owned()];
    excluded_expected.sort();
    assert_eq!(
        excluded_seen, excluded_expected,
        "the two non-override declaration sites must both still be found; if \
         one moved, update TRAIT_DEFAULT / REGISTRY_DELEGATE rather than \
         leaving an exclusion that matches nothing"
    );

    found.sort();
    let mut registered: Vec<String> = BACKENDS_THAT_DECLARE_LIMITATIONS
        .iter()
        .map(|b| b.source_file.to_owned())
        .collect();
    registered.sort();

    assert_eq!(
        found, registered,
        "a backend that declares a known limitation and is not in \
         `BACKENDS_THAT_DECLARE_LIMITATIONS` is a disclosure nothing grades: \
         #368 c6 was already graded `met` while exactly that hole let \
         `AppContainerBackend::known_limitations` be deleted with every test \
         green. Add the row here AND an arm in \
         `wcore_cli::sandbox_cmd::disclosure_tests`."
    );
}

/// The table's own rows must be well formed, or the comparison above can be
/// satisfied by a row that names a file which does not exist.
#[test]
fn every_registered_row_names_a_real_file_and_a_backend() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        !BACKENDS_THAT_DECLARE_LIMITATIONS.is_empty(),
        "an empty table makes every check over it vacuous"
    );
    for row in BACKENDS_THAT_DECLARE_LIMITATIONS {
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
    }
}
