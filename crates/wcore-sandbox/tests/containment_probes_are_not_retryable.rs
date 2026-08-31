//! Issue #362 c4: a containment test that never ran its probe must not be
//! retried into a pass.
//!
//! # What was measured, and why a panic was not enough
//!
//! `wcore_sandbox::test_support::run_contained_probe` already panics when the
//! backend refuses to spawn — "the sandbox backend refused to run the
//! containment probe, so no containment property was tested". In CI run
//! 33240249894 (linux-containerized) `bwrap_confines_filesystem_writes_outside_
//! allowlist` hit exactly that, out of the sandbox's own process-tree
//! ownership path, was retried under `[profile.ci] retries = 2`, passed on a
//! later attempt, and the run conclusion said SUCCESS.
//!
//! So the loud panic reached nobody. A retried ASSERTION failure at least means
//! the assertion ran; a retried PROBE REFUSAL means the run reported a security
//! property it never measured. `.config/nextest.toml` therefore pins these
//! binaries to `retries = 0`, and this file is what keeps that pin honest.
//!
//! # Why a scan and not a list
//!
//! A hand-maintained list closes today's hole and lets the next containment
//! binary reopen it silently — the same shape `#368` c6 was refiled over. The
//! undecidable question ("did somebody remember to pin this binary?") is
//! replaced by a decidable one: *is every test binary in this crate that calls
//! `run_contained_probe` covered by a `retries = 0` override?*

use std::path::{Path, PathBuf};

/// The helper whose refusal is the vacuity this file exists to keep visible,
/// matched in its CALL form so a file that merely names it in prose is not
/// mistaken for a caller.
const PROBE: &str = concat!("run_contained_", "probe(");

/// This file's own stem. It is skipped, and skipped EXPLICITLY rather than by
/// hoping its prose never matches: a scanner that grades itself would either
/// demand a pin for a source-only ratchet with no process in it, or force its
/// own documentation to avoid naming the thing it is about.
const SELF: &str = "containment_probes_are_not_retryable";

fn workspace_root() -> PathBuf {
    // crates/wcore-sandbox -> crates -> <root>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two levels under the workspace root")
        .to_path_buf()
}

/// Every integration-test binary in this crate that drives a containment probe.
/// The binary name is the file stem, which is what nextest's `binary(=...)`
/// predicate matches.
fn probe_binaries() -> Vec<String> {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&tests).expect("this crate has a tests/ directory");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_stem().is_some_and(|s| s == SELF) {
            continue;
        }
        if path.extension().is_some_and(|e| e == "rs")
            && std::fs::read_to_string(&path).is_ok_and(|t| t.contains(PROBE))
        {
            out.push(
                path.file_stem()
                    .expect("a .rs file has a stem")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    out.sort();
    out
}

/// The `binary(=NAME)` operands of every `[[profile.ci.overrides]]` block that
/// sets `retries = 0`.
///
/// Parsed rather than string-searched for a reason recorded in this file's
/// sibling blocks: `.config/nextest.toml` already carries `retries = 0` blocks
/// under `[profile.default.overrides]`, and CI runs `--profile ci`. A whole-file
/// substring match would accept a pin that never applies to the run that
/// matters, which is a mistake this repo has already made once (see the
/// `deterministic_openai_loop` note in that file).
fn ci_zero_retry_binaries(toml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_ci_override = false;
    let mut filter = String::new();
    let mut zero = false;
    let flush = |filter: &mut String, zero: &mut bool, out: &mut Vec<String>| {
        if *zero {
            let mut rest = filter.as_str();
            while let Some(i) = rest.find("binary(=") {
                rest = &rest[i + "binary(=".len()..];
                if let Some(end) = rest.find(')') {
                    out.push(rest[..end].trim().to_owned());
                    rest = &rest[end..];
                } else {
                    break;
                }
            }
        }
        filter.clear();
        *zero = false;
    };
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            flush(&mut filter, &mut zero, &mut out);
            in_ci_override = trimmed == "[[profile.ci.overrides]]";
            continue;
        }
        if !in_ci_override || trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("filter") {
            filter.push_str(value);
        } else if trimmed.replace(' ', "") == "retries=0" {
            zero = true;
        }
    }
    flush(&mut filter, &mut zero, &mut out);
    out.sort();
    out.dedup();
    out
}

#[test]
fn every_containment_probe_binary_is_pinned_to_zero_retries() {
    let binaries = probe_binaries();
    // POSITIVE CONTROL on the source scan. A drifted helper name or an
    // unreadable tests/ dir would leave an empty list, and an empty list is
    // covered by any pin at all — the vacuity this file exists to close.
    assert!(
        binaries.len() >= 2,
        "the scan found {binaries:?} calling the containment-probe helper; this \
         crate has more than one \
         containment-probe binary, so the scanner is not reading the tests"
    );

    let path = workspace_root().join(".config/nextest.toml");
    let toml = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the nextest config must be readable at {path:?}: {e}"));
    let pinned = ci_zero_retry_binaries(&toml);
    // POSITIVE CONTROL on the parser. If the block grammar changed under it,
    // it would return nothing and blame the config instead of itself.
    assert!(
        pinned.len() >= 3,
        "the parser found only {pinned:?} pinned to `retries = 0` under \
         `[profile.ci.overrides]` in {path:?}; that file carries several such \
         blocks, so the parser is broken rather than the config"
    );

    let missing: Vec<&String> = binaries.iter().filter(|b| !pinned.contains(b)).collect();
    assert!(
        missing.is_empty(),
        "test binaries {missing:?} run a containment probe and are NOT pinned to \
         `retries = 0` under `[profile.ci.overrides]` in {path:?}. Under \
         `[profile.ci] retries = 2` a backend that refuses to START the probe is \
         retried, and a later attempt that succeeds makes the run conclusion say \
         SUCCESS over a containment property that was never measured — MEASURED, \
         CI run 33240249894, FerroxLabs/wayland-core#362 c4. Add the binary to the \
         `#362 c4` override block."
    );
}

/// The parser's own discriminating control: a `retries = 0` block under
/// `[profile.default.overrides]` must NOT be read as a CI pin. CI runs
/// `--profile ci`, and a default-profile pin does not apply to it.
#[test]
fn a_default_profile_pin_is_not_mistaken_for_a_ci_pin() {
    let toml = "\
[[profile.default.overrides]]
filter = 'binary(=only_pinned_by_default)'
retries = 0

[[profile.ci.overrides]]
filter = 'binary(=pinned_for_ci)'
retries = 0

[[profile.ci.overrides]]
filter = 'binary(=ci_but_retried)'
retries = 2
";
    assert_eq!(
        ci_zero_retry_binaries(toml),
        vec!["pinned_for_ci".to_owned()]
    );
}
