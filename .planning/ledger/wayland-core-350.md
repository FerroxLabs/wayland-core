---
issue: 350
repo: FerroxLabs/wayland-core
kind: defect
title: "[nightly-windows-soak] FAIL - 2026-08-28"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The latency half of the streaming-timeout test refuses to grade when the derived timeout does not dominate the host's own measured timer allowance"
    state: met
    evidence: "symbol:crates/wcore-tools/src/bash/tests.rs::decisive_walk_floor"
    owner: core
    note: "Two changes, both present: timeout_ms is floored at a MEASURED timer allowance (tests.rs:2894-2900), and the latency half gates on floor <= decisive with decisive = (timeout+allowance)*2, the SAME expression the attribution half already used - so the factor was not widened. The allowance is bracketed before AND after the call and the larger is used."
  - id: c2
    text: "On a host that cannot establish the premise, the test skips loudly with the measured numbers instead of panicking"
    state: met
    evidence: "test:crates/wcore-tools/src/bash/tests.rs::the_streaming_bash_timeout_bounds_the_secret_deny_walk"
    owner: core
    note: "On the last attempt it prints SKIP (wayland-core#350) carrying timeout_ms, the measured walk floor, decisive and the measured allowance, then returns. No panic. Earlier attempts grow the tree and re-race."
  - id: c3
    text: "A positive control on a fine-tick host still turns red when the manifest build is moved back outside the timeout scope"
    state: met
    evidence: "symbol:crates/wcore-tools/src/bash/tests.rs::decisive_walk_floor"
    owner: core
    note: "POSITIVE CONTROL RUN on hetzner-dsm 2026-08-29, and the host is fine-tick BY THE TEST'S OWN MEASUREMENT inside the failing run: a 4.096452ms timer allowance, against the 15.809ms the source comment records for the self-hosted Windows runner. Unmutated baseline: `PASS [   5.204s]` with an EMPTY stderr, i.e. no `SKIP (wayland-core#350)` line -- it graded rather than skipping. Mutation, in crates/wcore-tools/src/bash.rs::execute_streaming_with_ctx only: the manifest build's deadline was replaced with now + 3600s and the real `now + timeout` deadline restarted AFTER the build, which is the pre-#1111 shape the criterion names. The diff was read back; both edits landed on `let deadline = ...` CODE statements. Red arm, verbatim: `thread 'bash::tests::the_streaming_bash_timeout_bounds_the_secret_deny_walk' (3598533) panicked at crates/wcore-tools/src/bash/tests.rs:2974:9: / a 3ms streaming timeout floors at 35.341322ms over 3 calls, at or above the 14.192904ms this host's deadline alone can cost (timeout + a measured 4.096452ms timer allowance, doubled), against a walk flooring at 32.031928ms -- the manifest build is outside the timeout scope`. CONTROL IN THE SAME INVOCATION: the buffered path was deliberately left unmutated and `PASS [   3.564s] (1/2) wcore-tools bash::tests::the_bash_timeout_bounds_the_secret_deny_walk` in the same run, so the red is attributable to the moved build and not to a broken instrument or a loaded box. `Summary [   3.602s] 2 tests run: 1 passed, 1 failed, 1810 skipped`. bash.rs was restored to a clean git diff afterwards."
  - id: c4
    text: "The retries=0 override on this test survives the fix"
    state: met
    evidence: "file:.config/nextest.toml:599"
    owner: core
    note: "the override naming this test by literal operand is correct and should be kept - the failure must not become retryable. A separate script fails if any of the three named tests stops being a literal operand of a retries=0 predicate"
  - id: c5
    text: "The issue's own close condition is met: a green nightly-windows-soak run against this tree"
    state: superseded
    evidence: "test:crates/wcore-tools/src/bash/tests.rs::the_streaming_bash_timeout_bounds_the_secret_deny_walk"
    owner: core
    note: "SOAK RUN AGAINST THIS TREE 2026-08-29: https://github.com/FerroxLabs/wayland-core/actions/runs/33258858506 (workflow_dispatch on lane/f13-fin-windows-runs at bd184563). NOT GREEN - and the reason is the whole finding, so read the next two paragraphs before reading the conclusion. THIS ISSUE'S OWN DEFECT IS FIXED AND THE SOAK PROVES IT. #350 was filed because `the_streaming_bash_timeout_bounds_the_secret_deny_walk` failed on BOTH attempts of the 2026-08-28 nightly. In this run, on `windows-2025`, PHASE G reports it verbatim as `SLOW [> 30.000s]` then `PASS [  49.842s] (3060/4123) wcore-tools bash::tests::the_streaming_bash_timeout_bounds_the_secret_deny_walk`. It graded, on the coarse-tick host class the c2 skip arm exists for, and it passed. That is the close condition's substance. WHAT KEPT THE RUN RED IS TWO OTHER DEFECTS, BOTH NOW FILED, NEITHER THIS ONE. (1) The self-hosted msvc `Windows live-acceptance` job failed PHASE L on `concurrent_allow_and_deny_identities_do_not_interfere`, verbatim `thread 'concurrent_allow_and_deny_identities_do_not_interfere' (20672) panicked at crates\\wcore-sandbox\\tests\\live_fs_acl.rs:471:5: / ordinary allow identity must retain access`, twice, then `x PHASE L failed: live_fs_acl` - that is wayland-core#324, measured and root-caused this same day and carried to FerroxLabs/wayland-core#368. (2) PHASE G's single hard failure in 3060 tests, 3 of 3 tries, was `wcore-tools path_validation::tests::a_path_whose_metadata_fails_for_a_reason_other_than_absence_is_refused` - a #238 c6 test whose Unix ENOTDIR provocation maps to `NotFound` on Windows so it cannot establish its own premise, filed as FerroxLabs/wayland-core#374 and reproduced on the workstation at retries=0. `a_cancelled_streaming_bash_does_not_wait_for_the_secret_deny_walk` was FLAKY 2/2 (TRY 1 FAIL, TRY 2 PASS) and did not fail the phase. SO: this criterion is a whole-run predicate over defects it does not own, and it is DECOMPOSED onto the two that own them - #368 and #374. It becomes true when they do, with no further work on #350 itself. The MASTER-PLAN.md:373 residual is unchanged and still unclaimed: it asks the latency half be confirmed ABSENT from the Windows nextest list rather than self-skipping, and as implemented it self-skips - though note this run did not exercise that path, because the host graded rather than skipping. BLOCKER LIST HALVED 2026-08-29 on lane/f13-u-win-native: of the two defects that kept run 33258858506 red, #374 is now CLOSED - the PHASE G hard failure `path_validation::tests::a_path_whose_metadata_fails_for_a_reason_other_than_absence_is_refused`, the only hard failure in 3060 tests, now passes on Windows 11 build 26200 with its premise genuinely established. #368 is the SOLE remaining blocker on this criterion, and it now has a ledger entry with gradeable criteria where before it had none. SAID PLAINLY so it is not re-derived: no green soak has been obtained, and one cannot be until #368 lands, because the self-hosted msvc `Windows live-acceptance` job fails PHASE L on `concurrent_allow_and_deny_identities_do_not_interfere` on roughly one run in five. This criterion is a whole-run predicate over a defect it does not own and stays superseded onto #368 alone."
---

This is an auto-filed nightly soak failure. Its body says to grep the build,
nextest and mutants sections, which is boilerplate and is wrong here: the run
log shows a single test failing on both attempts,
the_streaming_bash_timeout_bounds_the_secret_deny_walk in wcore-tools.

It is not stale and it is not the AppContainer ACL race. The failing lines are
byte-identical at the shipped tree - the ship diff to that test file is 77 added
lines, all appended well below them - so this reproduces on v0.13.10.

The root cause is that the instrument is grading below its own noise floor. The
timeout under test is derived as one tenth of a measured walk, which came out at
6 to 7 milliseconds on a host whose timer tick the test itself measured at 15.8
milliseconds. The residue being asserted on is quantization, not product
latency. The other half of the same test already knows this and refuses to grade
when the premise fails; this half does not.

One arm of the fix is verifiable on hetzner, the positive control. The skip arm
needs a coarse-tick host. Criteria come from the cluster C verification note of
2026-08-29.
