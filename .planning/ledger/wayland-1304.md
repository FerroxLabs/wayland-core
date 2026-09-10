---
issue: 1304
repo: FerroxLabs/wayland
kind: defect
title: "the_streaming_bash_timeout_bounds_the_secret_deny_walk hard-fails ci-linux at ~1 in 9: the manifest walk dominated the deadline and the caller was not told"
status: open
last_verified_commit: 676336306
criteria:
  - id: c1
    text: "The path that returns a message NOT naming the manifest, while the manifest walk dominated the deadline, is identified BY FRAME rather than inferred. The two candidates are the child-timeout path returning first, and the manifest-build path losing its own attribution."
    state: met
    evidence: "commit:dafa1aa5c"
    owner: core
    note: "MET at 7e0a105a4 BY FRAME: the frame produced the message rather than being argued to. The evidence commit 90bef1c9c is the RED ARM -- the deterministic reproducer and the poll-stall seam, with the product decision NOT yet wired. Against that tree, on hetzner-dsm in proof slot parallel-1, `cargo test -p wcore-tools --lib`: `a_manifest_build_that_returns_after_the_deadline_still_names_the_scan` FAILED with `got: Command timed out after 13ms (streaming path, a 13ms timeout against a walk measured at 26.261776ms, stalled 500ms before the first poll)`. That byte sequence is produced at exactly ONE site -- the SECOND `tokio::time::timeout_at(deadline, run)` in `execute_streaming_with_ctx` -- and that site is reachable only when the FIRST `timeout_at(deadline, build)` returned `Ok`, i.e. when the build became ready at or after the deadline and tokio`s poll-inner-before-deadline order let it win. CANDIDATE 1 (the child-timeout path returning first) CONFIRMED. CANDIDATE 2 (the manifest-build path losing its own attribution) REFUTED by the same frame: that arm`s message names the manifest verbatim (`while building the sandbox manifest (the workspace secret-scan); the command never ran`) and is not what came back. LIMIT, STATED: this identifies the mechanism on a reproducer, not the historical CI event of run 33708958434, which was not frame-captured and cannot now be. What it establishes is that the mechanism exists, is the only one producing that string, and is reachable with the walk dominating the deadline. ORIGINAL FILING 2026-09-03 from a hard ci-linux failure on PR #426, run 33708958434, bash/tests.rs:2532; the premise-resistance design described there is unchanged and still grades the property in situ."
  - id: c2
    text: "A regression test fails DETERMINISTICALLY for that path rather than by racing a wall clock, so the fix is verifiable without waiting for a 3-10 percent event."
    state: met
    evidence: "test:crates/wcore-tools/src/bash/tests.rs::a_manifest_build_that_returns_after_the_deadline_still_names_the_scan"
    owner: core
    note: "MET at 7e0a105a4. The frame is a POLL ORDER, not a race, so it can be reached on demand: the test spawns the manifest build and then stalls the runtime thread for 10x the measured walk (>=500ms) before the select is first polled, so the build is finished and the deadline long past when `timeout_at` looks at it. Two facts of the fixture rather than two hopes -- `timeout` is derived at HALF the measured walk and that measurement is warm and therefore a LOWER bound on what the build under test pays, so `build_took >= timeout` cannot fail to hold; and the stall is an order of magnitude longer than that walk. ARMS on hetzner-dsm in proof slot parallel-1, `cargo test -p wcore-tools --lib`: FAIL-BEFORE at 90bef1c9c (fix withdrawn at both call sites, seam and test present) 1 passed 1 FAILED, payload quoted in c1; PASS-AFTER at 12f9d1094 93 passed 0 failed across all of bash::tests, both racing tests included. It grades BOTH exec paths -- streaming and buffered -- in one loop, and it asserts the SPECIFIC post-deadline wording, so the pre-deadline `Err` arm (which also names the manifest) cannot satisfy it vacuously. The P2b unsaved-work guard is asserted against rather than absorbed: its budget is left room by deriving the timeout at half the walk, and if it ever spends the budget first the test reds naming it as an instrument failure. LIMITS: this does not measure the field rate -- c4 still records that as unmeasured -- and the stall seam is `#[cfg(test)]` and thread-local, so it cannot exist in a production build nor leak into a test running beside it."
  - id: c3
    text: "A green run is distinguishable from a vacuous one IN CI. The non-grading path prints `SKIP (#319)` to stderr; ci.yml runs `cargo nextest run --workspace --profile ci --no-fail-fast` with the default `success-output = never`, and nextest captures a passing test's output, so the disclosure reaches nobody on exactly the runs where it matters."
    state: met
    evidence: "file:.config/nextest.toml:837:a green here must be distinguishable from a vacuous one"
    owner: core
    note: "MET at 7e0a105a4, PROVEN BY A/B rather than by reading the config. Two changes: the GRADING path now prints a positive receipt (`GRADED (#1111 acceptance 3) ...` naming the deadline, the walk and the message) beside the existing SKIP lines, so a green that graded the criterion and a green that graded nothing no longer say the same thing; and a per-test `success-output` override carries whichever line was printed out of the runner. MEASURED on hetzner-dsm in proof slot parallel-1, both arms `cargo nextest run --workspace --profile ci --retries 0 --no-fail-fast` -- the exact profile ci.yml uses. WITH the override: `2 tests run: 2 passed`, and 2 `GRADED (#1111 acceptance 3)` lines in the run log. CONTROL, same tree, `--success-output never` (which is what the ci profile default gave these tests before this block): `2 tests run: 2 passed`, ZERO GRADED lines. Same green, no disclosure -- the defect, reproduced and then closed. The override sets ONLY `success-output`; the retries=0 override above it is untouched, and `scripts/check-windows-attribution.py` still sees its three literal operands. LIMIT: this puts the line in the log, it does not make anyone read it, and nothing in CI fails on a SKIP -- deliberately, because #319 is what failing on one costs."
  - id: c4
    text: "NOT MEASURED, and recorded as such: the rate. It needs the failing environment, or a deterministic reproducer from c2."
    state: met
    evidence: "file:.planning/ledger/wayland-1304.md"
    owner: core
    note: "MET at 509f4426b BY RECORD, and the record is quantitative rather than a shrug. In CI: 1 hard failure across the 9 ci-linux runs graded in the window where `report` failed continuously. On hetzner-dsm INSIDE the wayland-core-ci:rust-1.95-slim-bookworm image, with the same grants and env ci.yml gives the test step, at --retries 0: n=25, 0 failures, 2.699-3.381s. That green was checked for vacuity rather than assumed -- re-run with --success-output immediate, no `SKIP (#319)` appears on stderr and the stdout section IS shown, so the criterion was genuinely graded on every run. 0/25 bounds the rate near 11 percent upper and does not refute a ~10 percent one; combined with 1/9 in CI the rate is order 3-10 percent and is NOT established. Recorded as such, which is what this criterion asks for. EVIDENCE TOKEN IS DELIBERATELY THE BARE FILE FORM AND IT IS WEAK, stated rather than dressed up. This test is on neither allowlist by design (see c5), so no other file in the tree carries the record, and a file:<path>:<line>:<text> self-anchor is structurally impossible: the token text lands in the file it points at, the fragment matches twice, and the gate refuses it. The bare form proves only that this file exists, which is the strongest thing the grammar can say about a record kept in the ledger itself."
  - id: c5
    text: "Recorded, and deliberately not acted on: an intermittent hard failure has no home in either allowlist, and that is correct."
    state: met
    evidence: "absent:.config/flaky-allowlist.txt::the_streaming_bash_timeout_bounds_the_secret_deny_walk"
    owner: core
    note: "MET at 509f4426b. The non-action is real and it is anchored: the test appears ZERO times in .config/flaky-allowlist.txt, and the absence is controlled by that file existing and demonstrably carrying other entries, so an empty result is the entry being absent rather than the query being broken. The reason is recorded and is a property of the two mechanisms, not a preference. The retry allowlist matches only `<flakyFailure>`, i.e. a test retried into a pass, and this test carries `retries = 0` in .config/nextest.toml deliberately, so it can never produce one. The failing-set allowlist names a set that MUST fail, and a listed test that passes counts as STALE, which grade-failing-set.sh fails the run on. Both refuse an intermittent hard failure by design, so the only available disposition is to fix it -- which is what c1 and c2 owe. WHAT WOULD FALSIFY THIS: an allowlist entry appearing for this test, which the token reds on."
---

# The test is not flaking. It is reporting.

`the_streaming_bash_timeout_bounds_the_secret_deny_walk` panics only when the
manifest walk DOMINATED the deadline -- minimum of three samples, against a
threshold of twice the deadline's honest upper bound -- and the message that came
back still did not name the manifest.

Every mechanism this repository has for absorbing a red refuses this one, on
purpose. A retry cannot launder it, an allowlist cannot hold it, and a count
cannot hide it. What is left is the fix.
