//! core#362 c4: the `retries = 0` override must keep covering every
//! containment probe, including the ones written after it.
//!
//! `run_contained_probe` (`src/test_support.rs`) panics rather than returning
//! a degraded result when the backend refuses to spawn, when the child emitted
//! no begin marker, or when it died part-way. Each of those panics means the
//! same thing — NO CONTAINMENT PROPERTY WAS TESTED — and under `[profile.ci]
//! retries = 2` each is laundered into a green run conclusion by a retry. CI
//! run 33240249894 is the measured instance.
//!
//! `.config/nextest.toml` closes that for the two binaries that call the
//! helper TODAY. An override is a list of names; it cannot notice a third
//! caller. This is the ratchet that does, and it is a source check with no
//! process in it, so nothing here can itself be retried into meaninglessness.
//!
//! It deliberately checks the LITERAL operands of `binary(=...)` predicates
//! rather than resolving the filterset, exactly as
//! `scripts/check-windows-attribution.py` does: resolving needs
//! `cargo nextest list`, and a check that needs the test harness to be healthy
//! cannot grade the test harness.

use std::path::{Path, PathBuf};

/// This test binary's own file stem. See `probe_binaries`.
const RATCHET_BINARY: &str = "probe_retry_ratchet";

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/wcore-sandbox.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root")
        .to_path_buf()
}

/// Every `crates/wcore-sandbox/tests/*.rs` binary whose source calls
/// `run_contained_probe`.
fn probe_binaries(root: &Path) -> Vec<String> {
    let dir = root.join("crates/wcore-sandbox/tests");
    let mut found: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_owned();
            // THIS file is excluded, and the exclusion is the finding rather
            // than a convenience. It names the helper in a string literal in
            // order to SEARCH for it, so the first version of this scan matched
            // itself and demanded that the ratchet be pinned to retries = 0 —
            // a source check with no process in it, which a retry cannot
            // launder. A grader is not a caller. Nothing else is excluded, and
            // the positive controls below fail if this exclusion ever starts
            // swallowing a real caller.
            if stem == RATCHET_BINARY {
                return None;
            }
            let source = std::fs::read_to_string(&path).ok()?;
            // The helper's own name in prose is not a call. Require the opening
            // paren, which a doc mention does not carry.
            if !source.contains("run_contained_probe(") {
                return None;
            }
            Some(stem)
        })
        .collect();
    found.sort();
    found
}

/// Every name appearing as a literal `binary=<name>` operand inside a
/// `[[profile.ci.overrides]]` block that sets `retries = 0`.
///
/// Comment lines are stripped first: this file's own prose names both
/// binaries, and a check that reads its own documentation as configuration
/// passes for the wrong reason.
fn binaries_pinned_to_zero_retries(root: &Path) -> Vec<String> {
    let path = root.join(".config/nextest.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));

    let mut pinned = Vec::new();
    let mut filter: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("[[profile.") {
            filter = None;
        } else if let Some(rest) = line.strip_prefix("filter = ") {
            filter = Some(rest.trim().trim_matches('\'').trim_matches('"').to_owned());
        } else if line.starts_with("retries") && line.ends_with('0') {
            // `retries = 0` closes whichever filter is in scope.
            if let Some(filter) = filter.as_deref() {
                for chunk in filter.split("binary(=").skip(1) {
                    if let Some(name) = chunk.split(')').next() {
                        pinned.push(name.trim().to_owned());
                    }
                }
            }
        }
    }
    pinned.sort();
    pinned.dedup();
    pinned
}

#[test]
fn the_probe_binaries_cannot_be_retried_into_a_pass() {
    let root = workspace_root();
    let probes = probe_binaries(&root);

    // POSITIVE CONTROL FIRST. An empty scan reads exactly like "every binary
    // is covered", so a broken enumeration would make this test pass while
    // proving nothing — the same vacuity #362 c4 is about.
    assert!(
        probes.contains(&"backend_integration".to_owned()),
        "instrument check: backend_integration calls run_contained_probe and must be found. \
         found={probes:?}"
    );
    assert!(
        probes.len() >= 2,
        "instrument check: at least two binaries in this crate call run_contained_probe. \
         found={probes:?}"
    );

    let pinned = binaries_pinned_to_zero_retries(&root);
    assert!(
        pinned.contains(&"backend_integration".to_owned()),
        "instrument check: the .config/nextest.toml parse found no retries=0 override naming \
         backend_integration, so a missing binary below could not be distinguished from a \
         broken parse. pinned={pinned:?}"
    );

    let uncovered: Vec<&String> = probes.iter().filter(|b| !pinned.contains(b)).collect();
    assert!(
        uncovered.is_empty(),
        "these test binaries call run_contained_probe but are NOT pinned to retries = 0 in \
         .config/nextest.toml: {uncovered:?}\n\n\
         run_contained_probe PANICS when the backend refuses to start the probe, which means no \
         containment property was tested. Under [profile.ci] retries = 2 that panic is retried \
         and the run CONCLUSION reports SUCCESS over a sandbox test that proved nothing — CI run \
         33240249894, core#362. Add the binary to the `#362 c4` override block, or move its \
         probe call out.\n\
         probe callers={probes:?}\n  pinned={pinned:?}"
    );
}
