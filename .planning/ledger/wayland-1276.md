---
issue: 1276
repo: FerroxLabs/wayland
kind: defect
title: "No standing gate stops a FOURTH hand-cut URL authority parser (split from #1252 c3)"
status: open
last_verified_commit: a07bf29e5
criteria:
  - id: c1
    text: "Adding a function to `crates/` that returns a host- or authority-shaped value by string surgery rather than through `url::Url` / `wcore_types::url_authority` fails a gate rather than passing silently -- shown RED by adding one."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
  - id: c2
    text: "The gate`s enumeration is a total syntactic set over production sources, not a list of cutting idioms, and it carries an anti-vacuity control that fails CLOSED when its own enumeration stops matching (the `sites >= N` shape `wayland-core#402` established)."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
  - id: c3
    text: "Every site the enumeration finds is classified -- `answers through the parser`, `returns no host`, or `renders without deciding` -- with the reason recorded where the gate reads it, so the four already-dispositioned display cuts and `events.rs::split_endpoint` stay green without weakening it."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
  - id: c4
    text: "The three sites `#1252` fixed and the two `#1243` fixed are all seen by the enumeration -- the known-positive control, so a gate that finds nothing cannot pass."
    state: not-met
    owner: core
    note: "Transcribed verbatim from the issue body on 2026-08-31. not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task: the gate reserves task for a credential, an account or a platform a human must obtain, and there is code behind this one."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-release-readiness.py` reads ledger files and nothing else, so an open in-scope issue with no ledger is invisible to it. `check-criteria-ledger.py`'s
coverage arm is the only thing that reports the gap, and CI runs that arm
`--offline`, which cannot ask the trackers -- so nothing said so.

Criteria are transcribed from the issue body WITHOUT EDIT. Where the wording is
loose it is left loose: sharpening a criterion inside the ledger is how a
criterion quietly becomes an easier adjacent property. Whoever takes this
restates it on the ISSUE first.


Provenance carried over from the issue, because it changes how c1 may be graded:
the issue attaches NO red arm and says so -- the defect is a counterfactual about a
cut that does not exist yet, and the filing lane declined to manufacture one rather
than report a modelled failure as a measured one. c1's own text supplies the arm
(`shown RED by adding one`), so whoever takes this adds a cut, watches the gate red,
and removes it. The 24-hit sweep in the body IS measured, at `lane/f13-authority`
@ `488fbbae9`, and found no undispositioned production site -- so this is a
STANDING-GATE gap, not a live parsing defect.
