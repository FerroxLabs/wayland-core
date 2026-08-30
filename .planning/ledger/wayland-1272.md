---
issue: 1272
repo: FerroxLabs/wayland
kind: defect
title: "0.13.12 release board: all 32 blocking issues mapped to an owner (17 in flight, 15 unassigned)"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "Every issue in `check-release-readiness.py`'s blocking list appears on this board with a named owner, and the board is updated whenever the list changes. An unowned issue is a bug in this ticket."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "Tranche 3a is assigned to lanes and each of its 6 issues reaches CLOSED or DECOMPOSED — no issue filed by our own verification is left unowned because it was found late."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "Tranche 3b's 9 residuals are each either closed, decomposed with a `handoff:`, or explicitly recorded as a decision that is not core's to take."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c4
    text: "`wayland#1203` c3 and any other criterion requiring a **live run** is either measured on real hardware or restated as what the tree can actually grade — a criterion that cannot be run is as worthless as one that cannot fail."
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
