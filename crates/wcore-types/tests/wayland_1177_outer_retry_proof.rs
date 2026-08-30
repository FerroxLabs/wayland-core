//! ONE-OFF PROOF PROBE for FerroxLabs/wayland#1177 c1 -- NOT shippable code.
//!
//! c1 asks that "a failure on attempt 1 followed by a pass on attempt 2 leaves
//! evidence the required report check can read". The mechanism (the wrapper at
//! .github/scripts/run-tests-with-attempt-evidence.sh plus
//! .github/scripts/grade-retry-flakes.sh) has never once been exercised by a
//! real attempt-1 failure on a real runner: every green run to date contains
//! "outer-retry attempt 1" and ZERO occurrences of attempt 2, which proves only
//! that the retry path did not run.
//!
//! This test manufactures the missing condition. It reads the wrapper's own
//! attempt counter and fails while that counter says 1, so:
//!   attempt 1 -> nextest red -> wrapper preserves outer-attempt-1.xml
//!   attempt 2 -> counter reads 2 -> green -> final-status.txt = success
//!   report job -> grade-retry-flakes.sh sees a preserved attempt behind a
//!                 SUCCESS status -> OUTER_UNLISTED=1 -> required check RED.
//!
//! Delete this file once the run is recorded.

use std::path::PathBuf;

fn counter_path() -> PathBuf {
    // nextest runs a test with cwd = the crate root. The wrapper writes its
    // counter under the WORKSPACE root, two levels up from crates/<crate>.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/nextest/ci/outer-attempts/.attempt")
}

#[test]
fn wayland_1177_outer_retry_proof_probe() {
    let path = counter_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "wayland#1177 PROBE: no outer-attempt counter at {} ({e}). The wrapper \
             did not run, so this probe cannot demonstrate anything.",
            path.display()
        )
    });
    let attempt: u32 = raw
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("wayland#1177 PROBE: counter {raw:?} is not a number ({e})"));

    // KNOWN-POSITIVE CONTROL: the counter must be a real, advancing value. A
    // zero would mean the wrapper wrote nothing and this probe is grading its
    // own default rather than the retry.
    assert!(
        attempt >= 1,
        "wayland#1177 PROBE control: counter read {attempt}, so the wrapper never incremented it"
    );

    assert!(
        attempt >= 2,
        "wayland#1177 PROBE: DELIBERATE failure on outer-retry attempt {attempt}. \
         This test passes from attempt 2 onward. If you are reading this in a junit \
         report the outer retry preserved evidence, which is exactly c1."
    );
}
