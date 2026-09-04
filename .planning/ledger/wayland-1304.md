---
issue: 1304
repo: FerroxLabs/wayland
kind: defect
title: "the_streaming_bash_timeout_bounds_the_secret_deny_walk hard-fails ci-linux at ~1 in 9: the manifest walk dominated the deadline and the caller was not told"
status: open
last_verified_commit: 509f4426b
criteria:
  - id: c1
    text: "The path that returns a message NOT naming the manifest, while the manifest walk dominated the deadline, is identified BY FRAME rather than inferred. The two candidates are the child-timeout path returning first, and the manifest-build path losing its own attribution."
    state: not-met
    owner: core
    note: "Filed 2026-09-03 from a hard ci-linux failure on PR #426, run 33708958434, bash/tests.rs:2532. THIS IS NOT A FLAKY TEST AND THE DISTINCTION IS THE WHOLE TICKET. The panic is reached only when the premise is DECISIVELY established, and the test is built to resist the contention explanation three ways: walk_floor takes the MINIMUM of 3 samples so one stall cannot inflate it; the threshold is (timeout + allowance) * 2, so the walk must DOMINATE rather than merely exceed the deadline; and the samples are taken AFTER the run under test, so they are warmer and therefore faster than what the real call paid. Quoted payload: the walk floors at 70.263583ms against a 13.15749ms timer-fire allowance. The premise held and the message still did not name the manifest, which is wayland#1111 acceptance 3 not holding."
  - id: c2
    text: "A regression test fails DETERMINISTICALLY for that path rather than by racing a wall clock, so the fix is verifiable without waiting for a 3-10 percent event."
    state: not-met
    owner: core
    note: "The existing test establishes its premise by growing a tree and re-racing, which is the right design for grading the property in situ and the wrong instrument for verifying a fix. Without c2 any fix is graded by absence over many runs, and this repo has recorded what that costs."
  - id: c3
    text: "A green run is distinguishable from a vacuous one IN CI. The non-grading path prints `SKIP (#319)` to stderr; ci.yml runs `cargo nextest run --workspace --profile ci --no-fail-fast` with the default `success-output = never`, and nextest captures a passing test's output, so the disclosure reaches nobody on exactly the runs where it matters."
    state: not-met
    owner: core
    note: "Same failure mode this repo has already recorded three times for warn! under an unset RUST_LOG: the product had the information and discarded it before any reader. It matters here because the criterion can pass WITHOUT BEING GRADED, so a run of greens carries no evidence that #1111 acceptance 3 still holds."
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
