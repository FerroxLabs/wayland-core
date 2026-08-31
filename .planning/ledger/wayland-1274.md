---
issue: 1274
repo: FerroxLabs/wayland
kind: defect
title: "A superseded criterion's successor is the first #N in its note, so prose decides where a residual is tracked"
status: open
last_verified_commit: 07ee39f6
criteria:
  - id: c1
    text: "The successor is read from a dedicated field, not from prose. A note that mentions other issue numbers cannot change where a residual is tracked."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "EVERY existing `superseded` criterion is re-resolved under the new rule and any whose successor CHANGES is listed. 'How many other pointers were wrong' is answered by measurement — one instance was found by accident, which says nothing about the rest."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "A red arm in both directions: a criterion whose note mentions a closed issue before its real open successor mis-resolves under today's rule and resolves correctly after; and a genuinely-orphaned residual whose note mentions an open issue first is shown to be reported after the fix and silent before."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c4
    text: "The new field is admitted by the strict key allowlist (`CRIT_KEYS` in `check-criteria-ledger.py`) AND seen by `check-release-readiness.py`, which imports that parser deliberately so the two gates cannot drift into two grammars."
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
