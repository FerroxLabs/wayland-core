---
issue: 1271
repo: FerroxLabs/wayland
kind: defect
title: "an_unknown_window_sizes_the_skill_listing fails 3/3 in the CI container on its own non-vacuity precondition, on integ/f13 too"
status: closed
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "— the test's non-vacuity is established from something the test itself controls, not from whatever skills the host image happens to ship: it plants enough catalogue to overflow the smaller budget, or it skips with a stated reason when it cannot, or it asserts the budget number rather than the rendered length. Evidence: the test passing in the CI container AND on a developer host, from one run of each."
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1150_unknown_context_window_test.rs::the_fixture_overflows_every_budget_under_test"
    owner: core
    note: "MET at 6e4eca07 by the same change that meets FerroxLabs/wayland-core#401 c1 and c3, and proven in the CI container by run 33637957153 job 100273473415, PASS ( 3498/17812). ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "— the property in (1) is still graded after the change, shown RED by a mutation that restores the 200,000 fabrication in `get_char_budget`'s `None` arm, with `cargo check -p wcore-agent --tests` exit 0 quoted so the red is a behaviour change and not a build failure."
    state: superseded
    successor: FerroxLabs/wayland-core#401
    owner: core
    note: "Handed to FerroxLabs/wayland-core#401 as its c5 when this issue closed as a duplicate. #401 is the carrier: same test, same file, same CI run 33291781675, filed 84 minutes EARLIER, and a superset of these criteria. Neither issue referenced the other in its body or comments, which is how the pair survived as two tickets. ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "— whatever the fix is, a catalogue smaller than the budget can no longer turn a real assertion into an environment-dependent failure: either the precondition is discharged by construction, or the test names the catalogue size it needs and fails with that number when it does not have it."
    state: met
    evidence: "file:crates/wcore-agent/tests/issue_1150_unknown_context_window_test.rs:141:const FILLER_DESC_LEN: usize = 400;"
    owner: core
    note: "MET at 6e4eca07. The precondition is now discharged BY CONSTRUCTION: 30 x 400 = 12,000 planted characters against an 8,000-char largest budget, asserted by the_fixture_overflows_every_budget_under_test rather than assumed. ORIGINAL: Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
