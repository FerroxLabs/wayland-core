//! TEMPORARY RED ARM for FerroxLabs/wayland#1177 c1. NOT FOR MERGE.
//!
//! c1 reads: "A failure on attempt 1 followed by a pass on attempt 2 leaves
//! evidence the required `report` check can read."
//!
//! Every previous attempt to grade that criterion substituted a weaker
//! property: a green CI run in which the wrapper printed `outer-retry attempt
//! 1` and nothing else. Attempt 1 passing means the retry path never executed,
//! so nothing was preserved and nothing was read -- it is evidence that the
//! `mkdir` blocker is gone and evidence for nothing else.
//!
//! This file manufactures the missing condition. It fails deterministically on
//! outer attempt 1 and passes on every later attempt, by reading the counter
//! `.github/scripts/run-tests-with-attempt-evidence.sh` maintains at
//! `<workspace>/target/nextest/ci/outer-attempts/.attempt`.
//!
//! It is INERT everywhere the wrapper has not run: if the counter file does not
//! exist, the test returns without asserting, so a developer running
//! `cargo nextest run` locally never sees it.

use std::path::{Path, PathBuf};

/// The wrapper's counter, or `None` when the wrapper has not run in this tree.
///
/// Resolved by walking up from this crate rather than from the process cwd:
/// nextest sets a test's cwd to its own package root, while the wrapper's
/// `ATTEMPT_DIR` is relative to the workspace root it was invoked from.
fn attempt_counter() -> Option<PathBuf> {
    let mut dir: &Path = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("target/nextest/ci/outer-attempts/.attempt");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

#[test]
fn outer_retry_evidence_probe_is_red_on_attempt_1_only() {
    let Some(counter) = attempt_counter() else {
        eprintln!(
            "wayland#1177 probe INERT: no outer-retry attempt counter under {}",
            env!("CARGO_MANIFEST_DIR")
        );
        return;
    };
    let raw = std::fs::read_to_string(&counter)
        .unwrap_or_else(|e| panic!("wayland#1177 probe: cannot read {}: {e}", counter.display()));
    let attempt: u32 = raw
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("wayland#1177 probe: counter {raw:?} is not a number: {e}"));

    assert!(
        attempt >= 2,
        "wayland#1177 c1 RED ARM, deliberate: this test fails on outer-retry attempt {attempt} \
         and passes from attempt 2 on. If you are reading this in the `report` job's \
         retry-flake grader, the mechanism works -- attempt 1's JUnit survived the retry that \
         used to overwrite it. Remove this file once the evidence is quoted on the issue."
    );
}
