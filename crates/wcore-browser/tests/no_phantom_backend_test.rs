//! Issue #113 — the shipped tree must not advertise a browser backend that
//! does not exist.
//!
//! #113 reported the Browser tool as "non-functional by default" and named a
//! `chromiumoxide` / CDP fallback "behind a non-default cargo feature and
//! partially stubbed (chromium.rs:265+)". There is no `chromium.rs`, no
//! `chromiumoxide` dependency and no such cargo feature anywhere in this
//! workspace: `wcore-browser` ships exactly two `BrowserProvider` impls,
//! `CamoufoxBackend` (primary) and `BrowserbaseBackend` (opt-in cloud, behind
//! the `browserbase` feature). The report was reading the crate's own stale
//! prose, which described a backend that was never built.
//!
//! Deleting that prose is only worth doing if it stays deleted, so this test
//! is the guard: it fails if any shipped `wcore-browser` source file claims a
//! chromiumoxide backend again. It scans source text rather than symbols on
//! purpose — the defect was a false CLAIM, and a claim lives in prose.

use std::path::{Path, PathBuf};

/// Collect every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_shipped_source_advertises_a_chromiumoxide_backend() {
    // `join` keeps this correct on Windows, where the separator differs.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);

    // POSITIVE CONTROL for the scanner itself: an empty or unreadable walk
    // would make the assertion below pass vacuously, which is exactly the
    // "a check that ran nothing" failure this repo keeps hitting.
    assert!(
        files.len() > 5,
        "the scanner found only {} files under {src:?} — it is not reading the crate",
        files.len()
    );
    assert!(
        files.iter().any(|f| f.ends_with("provider.rs")),
        "provider.rs — the file that carried the stale claim — was not scanned"
    );

    let mut offenders = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("read {file:?}: {e}"));
        for (i, line) in text.lines().enumerate() {
            if line.to_ascii_lowercase().contains("chromiumoxide") {
                offenders.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "wcore-browser advertises a chromiumoxide backend it does not have. \
         There are exactly two BrowserProvider impls in this crate — CamoufoxBackend \
         and BrowserbaseBackend. Delete the claim rather than the test:\n{}",
        offenders.join("\n")
    );
}

/// The two real backends must still BE there. Passes in both arms, and stops
/// the guard above from being satisfied by deleting the backends instead of
/// the false claim about them.
#[test]
fn the_two_real_backends_are_present() {
    let backends = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("backends");
    assert!(
        backends.join("camoufox.rs").is_file(),
        "the primary Camoufox backend is missing"
    );
    assert!(
        backends.join("browserbase.rs").is_file(),
        "the Browserbase cloud backend is missing"
    );
    assert!(
        !backends.join("chromium.rs").exists(),
        "a chromium.rs appeared — update this guard and #113's record together"
    );
}
