---
issue: 371
repo: FerroxLabs/wayland-core
kind: defect
title: "The UNIX half of the harness-ownership guard leaks the tree: harness_owns_spawned_trees fails 3/3 on integ/f13 and reddens every lane gate"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The Linux descendant snapshot (or the fixture's ordering, whichever the measurement indicts) is fixed, with which one established BEFORE either is changed"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c2
    text: "The Windows twin's anti-vacuity shape is kept: the grandchild is asserted to be inside what the guard owns BEFORE anything is killed, so the test cannot pass by measuring nothing"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.
