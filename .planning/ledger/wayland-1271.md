---
issue: 1271
repo: FerroxLabs/wayland
kind: defect
title: "an_unknown_window_sizes_the_skill_listing fails 3/3 in the CI container on its own non-vacuity precondition, on integ/f13 too"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "— the test's non-vacuity is established from something the test itself controls, not from whatever skills the host image happens to ship: it plants enough catalogue to overflow the smaller budget, or it skips with a stated reason when it cannot, or it asserts the budget number rather than the rendered length. Evidence: the test passing in the CI container AND on a developer host, from one run of each."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "— the property in (1) is still graded after the change, shown RED by a mutation that restores the 200,000 fabrication in `get_char_budget`'s `None` arm, with `cargo check -p wcore-agent --tests` exit 0 quoted so the red is a behaviour change and not a build failure."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "— whatever the fix is, a catalogue smaller than the budget can no longer turn a real assertion into an environment-dependent failure: either the precondition is discharged by construction, or the test names the catalogue size it needs and fails with that number when it does not have it."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
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
