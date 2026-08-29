---
issue: 361
repo: FerroxLabs/wayland-core
title: "Shared-process lib suite: active_approval_token_split_by_truncation_leaves_no_fragment fails its anti-vacuity control under load"
status: open
kind: defect
last_verified_commit: 0df4c47d
criteria:
  - id: c1
    text: "The mechanism is named: what makes output lack truncated under load is identified in code, not inferred"
    state: not-met
    owner: core
  - id: c2
    text: "The failure is reproduced deliberately at least once, with the command and environment recorded, before any fix is written"
    state: not-met
    owner: core
  - id: c3
    text: "The fixture reaches the truncation boundary deterministically, independent of scheduling"
    state: not-met
    owner: core
  - id: c4
    text: "Both assertions survive: the anti-vacuity control at mod.rs:5744 and the fragment assertion at :5749"
    state: not-met
    owner: core
  - id: c5
    text: "A red arm is quoted verbatim: the fixture failing before the change, from a real run"
    state: not-met
    owner: core
  - id: c6
    text: "After the fix, cargo test --workspace --lib --no-fail-fast passes N>=10 consecutive times on the build host, and the run count is recorded"
    state: not-met
    owner: core
---

Found by CI on PR #359 (the 0.13.11 version bump), and PROVEN not to be caused by it: the same
command is green on both the PR tree and on main at 20d99006, with the same total test count as
the failing CI run. Pre-existing, load- or scheduling-dependent.

The failing line is the anti-vacuity CONTROL, not the security assertion. The fixture did not
reach the truncation boundary, so the test refused to run its real assertion against a case that
was not the case it claims to test. No approval token was leaked and there is no customer-facing
exposure. The test behaved correctly by going red instead of passing vacuously.

nextest passes this test in the same job because it gives every test its own process. This leg
runs a crate lib tests in ONE process and exists precisely to catch what nextest cannot see, so
serialising or isolating the test would retire the instrument rather than fix the defect. That is
why c4 refuses any fix that relaxes the control.

Two hypotheses were tested and refuted, and one reproduction instrument was rejected as invalid:
constraining cores with taskset produces a DIFFERENT failure (a stack overflow in
concurrent_near_cap_admits_exactly_one_retained_workspace under thread contention), so it measures
the instrument rather than the subject. See the issue for detail.
