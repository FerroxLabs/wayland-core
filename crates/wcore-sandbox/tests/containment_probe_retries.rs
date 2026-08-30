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
//! produce a vacuous containment result — the source files that call the one
//! function that panics on one — and requires the config to cover it.
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
//!
//! # Why every file is CLASSIFIED rather than some files SKIPPED
//!
//! The first version of this file drew the boundary by directory name and got
//! it wrong. One helper read `crates/<crate>/tests/*.rs` non-recursively; its
//! companion walked `crates/` but skipped any directory called `tests`. The
//! union of the two is not the workspace — it omits
//! `crates/<crate>/tests/<subdir>/**`, which is 38 real files today including
//! `crates/wcore-sandbox/tests/common/mod.rs`, a shared helper in the very
//! crate being pinned. A caller planted there was covered by NEITHER test and
//! ran green with `retries = 2`, reproduced under review.
//!
//! Two hand-drawn regions that are meant to tile a space do not tile it, and
//! nothing in the code says so. So the shape changed rather than the region:
//! there is now ONE walk, it visits every `.rs` file under `crates/`, and each
//! file is CLASSIFIED — either into the integration binaries that can carry it
//! ([`Compiled::IntegrationBinaries`]) or into a target `binary(=<stem>)`
//! cannot name ([`Compiled::NotNameableByBinaryFilter`]). The classifier is
//! total: it returns a variant for every path, and a control asserts the two
//! buckets sum to the number of files walked. "Which directories do I skip"
//! is undecidable over an open alphabet of layouts; "every file lands in
//! exactly one of two buckets" is decidable and checkable, and the check runs.

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

/// Where a `.rs` file's code ends up, and therefore whether a nextest
/// `binary(=<name>)` operand can select it.
///
/// TOTAL over paths: [`classify`] returns one of these for every file it is
/// handed, and never `None`. That is the property the previous
/// skip-these-directories shape did not have.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Compiled {
    /// Into these integration-test binaries. More than one when the file is a
    /// module that any of a crate's test binaries may `mod`-include: the
    /// attribution is deliberately over-inclusive, because demanding a pin the
    /// file may not need costs a config line while missing one grants a silent
    /// exemption.
    IntegrationBinaries(Vec<String>),
    /// Into a crate's lib or bin target. A unit test there runs inside that
    /// target's own test binary, which `binary(=<file stem>)` cannot name.
    NotNameableByBinaryFilter,
}

/// The nextest integration-test binaries a crate's `tests/` directory produces:
/// `tests/<stem>.rs` is the binary `<stem>`, and `tests/<dir>/main.rs` is the
/// binary `<dir>` (cargo's two auto-discovery rules for test targets).
fn integration_binary_roots(crate_dir: &Path) -> Vec<String> {
    let tests = crate_dir.join("tests");
    let Ok(entries) = std::fs::read_dir(&tests) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.is_dir() {
            if path.join("main.rs").is_file() {
                out.push(name.to_owned());
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(name.trim_end_matches(".rs").to_owned());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Classify one `.rs` file living under `crates/`. `rel` is relative to
/// `crates/`, so its first component is the crate directory.
///
/// Every path yields a variant. A file under a crate's `tests/` subtree that
/// belongs to a crate with no integration binary at all cannot be named by
/// `binary(=...)` either, and is classified as such rather than being dropped —
/// the fail-closed direction, since that answer makes the file an OFFENDER
/// somebody has to deal with instead of an exemption nobody sees.
fn classify(crates_root: &Path, rel: &Path) -> Compiled {
    let parts: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    // <crate>/tests/... is the only integration-test shape.
    if parts.len() >= 3 && parts[1] == "tests" {
        let rest = &parts[2..];
        if rest.len() == 1 {
            return Compiled::IntegrationBinaries(vec![rest[0].trim_end_matches(".rs").to_owned()]);
        }
        let roots = integration_binary_roots(&crates_root.join(parts[0]));
        if roots.is_empty() {
            return Compiled::NotNameableByBinaryFilter;
        }
        return Compiled::IntegrationBinaries(roots);
    }
    Compiled::NotNameableByBinaryFilter
}

/// One walk, every `.rs` file under `crates/`, each classified.
///
/// `target` is excluded because it is build output rather than source — the
/// only exclusion, and it removes no source file. Everything that remains is
/// classified; nothing is skipped.
fn workspace_rs_files() -> Vec<(PathBuf, Compiled, String)> {
    let crates_root = repo_root().join("crates");
    let mut out = Vec::new();
    let mut stack = vec![crates_root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) != Some("target") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(&crates_root)
                .expect("walked from crates/")
                .to_path_buf();
            let compiled = classify(&crates_root, &rel);
            let body = std::fs::read_to_string(&path).expect("readable source");
            out.push((path, compiled, body));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
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

/// CONTROL shared by both tests: the walk is total.
///
/// Asserts the two buckets sum to the number of files walked, and that the
/// walk actually reaches a file in a `tests/` SUBDIRECTORY — the exact region
/// the previous two-helper shape omitted. If somebody restores a
/// skip-this-directory walk, this fires rather than silently exempting
/// whatever is behind it.
fn total_classification(files: &[(PathBuf, Compiled, String)]) {
    assert!(
        files.len() > 100,
        "CONTROL: the walk found only {} .rs files under crates/, so an empty result would \
         prove nothing. Did the layout move?",
        files.len()
    );
    let bucketed = files
        .iter()
        .filter(|(_, c, _)| {
            matches!(
                c,
                Compiled::IntegrationBinaries(_) | Compiled::NotNameableByBinaryFilter
            )
        })
        .count();
    assert_eq!(
        bucketed,
        files.len(),
        "CONTROL: classification must be TOTAL — every walked file lands in exactly one bucket. \
         {} of {} did.",
        bucketed,
        files.len()
    );
    let nested_test_files = files
        .iter()
        .filter(|(path, _, _)| {
            let parts: Vec<_> = path.components().collect();
            parts
                .iter()
                .position(|c| c.as_os_str() == "tests")
                .is_some_and(|i| parts.len() > i + 2)
        })
        .count();
    assert!(
        nested_test_files > 0,
        "CONTROL: the walk reached NO file in a `tests/` subdirectory. That region is exactly \
         what the previous non-recursive derivation missed, and a walk that cannot see it \
         cannot fail on it. Restore a recursive walk."
    );
}

#[test]
fn every_binary_that_can_report_a_vacuous_containment_probe_is_unretryable() {
    let files = workspace_rs_files();
    total_classification(&files);

    let mut binaries: Vec<String> = files
        .iter()
        .filter(|(_, _, body)| calls_the_vacuity_source(body))
        .filter_map(|(_, compiled, _)| match compiled {
            Compiled::IntegrationBinaries(names) => Some(names.clone()),
            Compiled::NotNameableByBinaryFilter => None,
        })
        .flatten()
        .collect();
    binaries.sort();
    binaries.dedup();

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

/// The remaining way to reach the vacuity source is from source that compiles
/// into a crate's lib or bin target rather than into a `tests/` binary of its
/// own. `binary(=<file stem>)` cannot name one, so the mechanism above cannot
/// cover it — and a mechanism that silently does not cover a case is the defect
/// this file exists to prevent.
///
/// Nothing does this today. This asserts that, so the day somebody does, the
/// failure says what to do instead of the pin quietly missing them.
#[test]
fn no_unit_test_reaches_the_vacuity_source_where_the_pin_cannot_name_it() {
    let files = workspace_rs_files();
    total_classification(&files);

    let offenders: Vec<&PathBuf> = files
        .iter()
        .filter(|(path, _, _)| !path.ends_with("src/test_support.rs"))
        .filter(|(_, compiled, _)| *compiled == Compiled::NotNameableByBinaryFilter)
        .filter(|(_, _, body)| calls_the_vacuity_source(body))
        .map(|(path, _, _)| path)
        .collect();

    assert!(
        offenders.is_empty(),
        "core#362 c4: {offenders:?} reach `{VACUITY_SOURCE}` from source that compiles into a \
         crate's lib or bin target rather than into a `tests/` binary. Such a test runs inside \
         that target's own test binary, which `binary(=<stem>)` cannot name, so the \
         `retries = 0` pin in .config/nextest.toml does NOT cover it and a vacuous containment \
         result there can still be retried into a pass. Move the call into a `tests/` binary and \
         pin that binary, or extend the pin mechanism to name lib binaries."
    );
}
