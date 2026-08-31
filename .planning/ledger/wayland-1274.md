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
    state: met
    evidence: "file:scripts/check-criteria-ledger.py"
    owner: core
    note: "`successor:` is a first-class field admitted by CRIT_KEYS, matched against `^<owner>/<repo>#<number>$`. A `superseded` criterion without one is a hard failure. The note is no longer read for this at all, so prose cannot decide where a residual is tracked."
  - id: c2
    text: "EVERY existing `superseded` criterion is re-resolved under the new rule and any whose successor CHANGES is listed. 'How many other pointers were wrong' is answered by measurement — one instance was found by accident, which says nothing about the rest."
    state: met
    evidence: "file:.planning/ledger/wayland-core-361.md"
    owner: core
    note: "ALL 16 superseded criteria in the tree re-resolved and migrated. THREE changed, which is the answer c2 asked for rather than an assumption that the one found by accident was the only one: wayland-core#350 c5 resolved to #350 (ITS OWN ISSUE), real successor #368; wayland-core#361 c6 resolved to #361 (ITS OWN ISSUE), real successor #373; wayland-core#338 c2 resolved to #380, which is CLOSED, because its note opens by discussing #380 -- real successor is the OPEN #389. A self-referential successor passed every check the gate had, because the issue exists and is open. Separately, the repo is now required: #370, #368, #373 and #389 all exist on BOTH trackers, and wayland-1155 c2 supersedes into wayland-core#370 while wayland#370 is a merged PR."
  - id: c3
    text: "A red arm in both directions: a criterion whose note mentions a closed issue before its real open successor mis-resolves under today's rule and resolves correctly after; and a genuinely-orphaned residual whose note mentions an open issue first is shown to be reported after the fix and silent before."
    state: met
    evidence: "file:scripts/check-criteria-ledger.py"
    owner: core
    note: "Six self-test arms, both directions, `self-test: both directions proven`. The two that carry the finding are new: a successor naming the criterion's OWN issue must redden, and a successor naming a tracker the number is not on must redden -- without the second, the repo half of the address could be ignored and every arm would still pass, which is the state the code was already in. One PRE-EXISTING arm was the defect in miniature: its note said the residual was carried by `#11 on the core tracker` while its fixture put 11 under wayland only, and it passed."
  - id: c4
    text: "The new field is admitted by the strict key allowlist (`CRIT_KEYS` in `check-criteria-ledger.py`) AND seen by `check-release-readiness.py`, which imports that parser deliberately so the two gates cannot drift into two grammars."
    state: met
    evidence: "file:scripts/check-release-readiness.py"
    owner: core
    note: "check-release-readiness.py imports this parser deliberately, so the two gates cannot drift into two grammars. After the change its self-test still reports `both directions proven` and its live verdict is unchanged at 43 issues / 143 criteria -- the field is visible to it and changed nothing it judges."
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
