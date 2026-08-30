---
issue: 406
repo: FerroxLabs/wayland-core
kind: defect
title: "Arm 3s gate cannot see a store created after the walk at a path it never recorded (residual of #390 c2)"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: ": A nested store created after the arm-3 walk, at a path that is neither store-shaped nor in the last scan's list, is refused on the next guard — measured with a wrong-refusal control in the same fixture, in the shape `vfs_guard_cost.rs::a_store_created_after_the_scan_is_denied_on_the_next_guard` already uses for arm 2."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: ": The cost of that closure is stated as a number and measured through `GuardCounters`, and whichever of core#390 c3 / core#398 c2 it moves is RE-GRADED rather than left claiming a figure the tree no longer has."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: ": `store_shaped`'s remaining role is decided explicitly — either it is the mutation net and the doc says so, or it is deleted because c1's mechanism subsumes it, and `tests/vfs_guard_cost.rs::a_gate_admitted_path_costs_one_probe_per_workspace_directory` is re-measured either way."
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
