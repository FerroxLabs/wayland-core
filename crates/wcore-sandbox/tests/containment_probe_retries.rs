//! core#362 c4 ANTI-ROT: every test that can end with a VACUOUS containment
//! result must be unretryable, and that must stay true for tests nobody has
//! written yet.
//!
//! `.config/nextest.toml` names two binaries in a `retries = 0` override. A
//! bare list is an enumeration, and an enumeration rots: the third caller of
//! [`VACUITY_SOURCE`] somebody adds next month inherits
//! `[profile.ci] retries = 2` and can be retried into a pass having proved no
//! containment property at all, which is the exact defect #362 records.
//!
//! So the list is not trusted. This test DERIVES the set of binaries that can
//! produce a vacuous containment result — the integration-test files that call
//! the one function that panics on one — and requires the config to cover it.
//!
//! It cannot resolve a nextest filterset, so it checks the literal spelling.
//! That is the same limitation, and the same remedy, as
//! `scripts/check-windows-attribution.py`.
//!
//! # Why the whole workspace and not this crate
//!
//! `wcore_sandbox::test_support` is `pub mod`, unconditionally, and SEVEN
//! crates depend on `wcore-sandbox`. A derivation that read only this crate's
//! `tests/` would have granted every one of them a silent exemption, which is
//! the same shape of gap as the nonce-scoped scan in core#366: a query that
//! cannot see the thing it is supposed to find.

use std::path::{Path, PathBuf};

/// The ONLY function that turns "the probe never ran" into a test failure.
/// Its two panic sites are what make an absent forbidden marker a failure
/// rather than a silent pass, so a file that calls it is a file whose failure
/// can mean "nothing was tested".
const VACUITY_SOURCE: &str = "run_contained_probe";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> has a workspace root two levels up")
        .to_path_buf()
}

/// True when `body` calls [`VACUITY_SOURCE`] from CODE rather than merely
/// naming it in prose.
///
/// # Why this does not simply search for the name
///
/// The first version of this file did, and it FAILED ON ITSELF: the doc
/// comments above quote the function, so the checker derived its own binary as
/// a caller. A search that matches prose is the same instrument error as a
/// mutation harness matching a doc comment that quotes the call it meant to
/// mutate.
///
/// So comment lines are dropped, and what is matched is CODE SYNTAX: a call
/// `run_contained_probe(`, or a path mention `::run_contained_probe` which is
/// how it is imported. Both are matched rather than only the call, because
/// over-inclusion here demands more coverage and under-inclusion silently
/// grants an exemption — the two errors are not symmetric.
///
/// NAMED GAP, since this is an allowlist and not a proof: a caller that reaches
/// the function through a re-export under a DIFFERENT name is not matched. A
/// caller that binds it to a local and invokes it through that binding
/// (`let probe = run_contained_probe; probe(..)` after a bare `use`) matches
/// the path form and is caught.
fn calls_the_vacuity_source(body: &str) -> bool {
    let call = format!("{VACUITY_SOURCE}(");
    let path_mention = format!("::{VACUITY_SOURCE}");
    body.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .any(|line| line.contains(&call) || line.contains(&path_mention))
}

fn read_rs_files(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("utf-8 file stem")
            .to_owned();
        out.push((
            stem,
            std::fs::read_to_string(&path).expect("readable source"),
        ));
    }
    out
}

/// Every `crates/<crate>/tests/*.rs` in the workspace, as `(binary name, body)`.
/// A file directly under `tests/` is one nextest binary whose name is the file
/// stem, which is exactly the operand `binary(=...)` takes.
fn workspace_integration_test_files() -> Vec<(String, String)> {
    let crates = repo_root().join("crates");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("the workspace has a crates/ directory") {
        let path = entry.expect("readable dir entry").path();
        out.extend(read_rs_files(&path.join("tests")));
    }
    out
}

/// Integration-test binary names anywhere in the workspace that call
/// [`VACUITY_SOURCE`].
fn binaries_that_can_report_a_vacuous_probe() -> Vec<String> {
    let mut out: Vec<String> = workspace_integration_test_files()
        .into_iter()
        .filter(|(_, body)| calls_the_vacuity_source(body))
        .map(|(name, _)| name)
        .collect();
    out.sort();
    out.dedup();
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

/// The remaining way to reach the vacuity source is from a UNIT test, which
/// lives in its crate's lib binary rather than in a `tests/` binary of its own.
/// `binary(=<file stem>)` cannot name one, so the mechanism above cannot cover
/// it — and a mechanism that silently does not cover a case is the defect this
/// file exists to prevent.
///
/// Nothing does this today. This asserts that, so the day somebody does, the
/// failure says what to do instead of the pin quietly missing them.
#[test]
fn no_unit_test_reaches_the_vacuity_source_where_the_pin_cannot_name_it() {
    let crates = repo_root().join("crates");
    let mut offenders = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![crates];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                // `tests/` is covered by the derivation above; `target` is not
                // source at all.
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != "tests" && name != "target" {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            // The definition itself is not a caller.
            if path.ends_with("src/test_support.rs") {
                continue;
            }
            let body = std::fs::read_to_string(&path).expect("readable source");
            scanned += 1;
            if calls_the_vacuity_source(&body) {
                offenders.push(path);
            }
        }
    }

    // CONTROL: a walk that found no files would make the assertion below
    // vacuous. `crates/` has hundreds of `.rs` files.
    assert!(
        scanned > 100,
        "CONTROL: the walk scanned only {scanned} .rs files under crates/, so an empty offender \
         list would prove nothing. Did the layout move?"
    );
    assert!(
        offenders.is_empty(),
        "core#362 c4: {offenders:?} reach `{VACUITY_SOURCE}` from library source rather than from \
         a `tests/` file. A unit test lives in its crate's lib binary, which `binary(=<stem>)` \
         cannot name, so the `retries = 0` pin in .config/nextest.toml does NOT cover it and a \
         vacuous containment result there can still be retried into a pass. Move the call into a \
         `tests/` binary and pin that binary, or extend the pin mechanism to name lib binaries."
    );
}
