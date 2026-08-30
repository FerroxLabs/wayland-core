//! core#362 c4 ANTI-ROT: every test that can end with a VACUOUS containment
//! result must be unretryable, and that must stay true for tests nobody has
//! written yet.
//!
//! `.config/nextest.toml` names two binaries in a `retries = 0` override. A
//! bare list is an enumeration, and an enumeration rots: the third caller of
//! [`run_contained_probe`] somebody adds next month inherits
//! `[profile.ci] retries = 2` and can be retried into a pass having proved no
//! containment property at all, which is the exact defect #362 records.
//!
//! So the list is not trusted. This test DERIVES the set of binaries that can
//! produce a vacuous containment result — the files under `tests/` that call
//! the one function that panics on one — and requires the config to cover it.
//!
//! It cannot resolve a nextest filterset, so it checks the literal spelling.
//! That is the same limitation, and the same remedy, as
//! `scripts/check-windows-attribution.py`.

use std::path::{Path, PathBuf};

/// The ONLY function that turns "the probe never ran" into a test failure.
/// Its two panic sites are what make an absent forbidden marker a failure
/// rather than a silent pass, so a binary that calls it is a binary whose
/// failure can mean "nothing was tested".
const VACUITY_SOURCE: &str = "run_contained_probe";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> has a workspace root two levels up")
        .to_path_buf()
}

/// Integration-test binary names in THIS crate that call [`VACUITY_SOURCE`].
///
/// # Why this does not simply grep for the name
///
/// The first version of this file did, and it FAILED ON ITSELF: the doc
/// comments above quote `run_contained_probe`, so the checker derived its own
/// binary as a caller. A search that matches prose is the same instrument
/// error as a mutation harness matching a doc comment that quotes the call it
/// meant to mutate.
///
/// So comment lines are dropped, and what is matched is CODE SYNTAX: a call
/// `run_contained_probe(`, or a path mention `::run_contained_probe` which is
/// how it is imported. Both are matched rather than only the call, because
/// over-inclusion here demands more coverage and under-inclusion silently
/// grants an exemption — the two errors are not symmetric.
///
/// NAMED GAP, since this is an allowlist and not a proof: a caller that binds
/// the function to a local and invokes it through that binding
/// (`let probe = run_contained_probe; probe(..)` after a bare `use`) matches
/// the path form and is caught; one that reaches it through a re-export under
/// a different name is not.
fn binaries_that_can_report_a_vacuous_probe() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let call = format!("{VACUITY_SOURCE}(");
    let path_mention = format!("::{VACUITY_SOURCE}");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("this crate has a tests/ directory") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let body = std::fs::read_to_string(&path).expect("readable test source");
        let uses_it = body
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .any(|line| line.contains(&call) || line.contains(&path_mention));
        if uses_it {
            out.push(
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .expect("utf-8 file stem")
                    .to_owned(),
            );
        }
    }
    out.sort();
    out
}

/// Every `[[profile.*.overrides]]` block that sets `retries = 0`, as its raw
/// filter line. Comment lines are dropped first: a name that appears only in
/// prose explaining the block would otherwise satisfy the check while
/// selecting no tests at all.
fn zero_retry_filters(config: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut profile = String::new();
    let mut filter = String::new();
    for raw in config.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("[[profile.") && line.ends_with(".overrides]]") {
            profile = line
                .trim_start_matches("[[profile.")
                .trim_end_matches(".overrides]]")
                .to_owned();
            filter.clear();
        } else if line.starts_with("filter =") {
            filter = line.to_owned();
        } else if line.replace(' ', "") == "retries=0" && !filter.is_empty() {
            out.push((profile.clone(), filter.clone()));
        }
    }
    out
}

#[test]
fn every_binary_that_can_report_a_vacuous_containment_probe_is_unretryable() {
    let binaries = binaries_that_can_report_a_vacuous_probe();

    // POSITIVE CONTROL. A scan that silently found nothing would make every
    // assertion below vacuously true, which is the failure mode this whole
    // file exists to prevent — and it has a precedent: an empty query reads as
    // "absent". If the helper is renamed, this fires instead of passing.
    assert!(
        binaries.len() >= 2,
        "CONTROL: the derivation found {} binary/binaries calling `{VACUITY_SOURCE}`. It must \
         find at least the two that exist (backend_integration, secret_read_deny); finding none \
         would make this test pass while proving nothing. Was the helper renamed?",
        binaries.len()
    );
    assert!(
        binaries.contains(&"backend_integration".to_string())
            && binaries.contains(&"secret_read_deny".to_string()),
        "CONTROL: the two known callers must be among {binaries:?}"
    );

    let path = repo_root().join(".config/nextest.toml");
    let config = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let filters = zero_retry_filters(&config);
    assert!(
        !filters.is_empty(),
        "CONTROL: no `retries = 0` override was parsed out of {} at all, so the checks below \
         could not fail whatever the config said",
        path.display()
    );

    // `[profile.default]` is what a bare `cargo nextest run` uses and
    // `[profile.ci]` is what every CI leg uses. A pin in only one of them has
    // already shipped in this file once; see the #1101 block there.
    for profile in ["default", "ci"] {
        for binary in &binaries {
            let operand = format!("binary(={binary})");
            let covered = filters
                .iter()
                .any(|(p, f)| p == profile && f.contains(&operand));
            assert!(
                covered,
                "core#362 c4: `{binary}` calls `{VACUITY_SOURCE}`, so it can fail with \"no \
                 containment property was tested\" — and under [profile.{profile}] it would be \
                 RETRIED, which is what turns that into a green run. Add `{operand}` to a \
                 `retries = 0` override for that profile in {}.\n  parsed zero-retry filters: \
                 {filters:#?}",
                path.display()
            );
        }
    }
}
