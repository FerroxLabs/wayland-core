---
issue: 367
repo: FerroxLabs/wayland-core
kind: defect
title: "A never-merge red-arm instrument reached integ/f13: OwnedTree owns the leaf only again on Unix"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The ten red-arm lines are out of integ/f13"
    state: met
    evidence: "commit:8df191706"
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass. VERIFIED AT HEAD by this pass: 8df191706 'Remove a red arm that shipped, and arm a gate that could never pass' deleted the ten lines, and cargo nextest run -p wcore-cli --test harness_owns_spawned_trees on hetzner reports Summary [0.227s] 24 tests run: 24 passed, 0 skipped."
  - id: c2
    text: "A red-arm instrument cannot be merged by accident: a test or CI grep fails when a shipped source file under crates/ contains black_box(true) or a RED ARM marker, with a positive control so the grep cannot pass by reading nothing"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c3
    text: "Whoever integrates NAMES the failing tests in a run rather than counting them"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
---

The ten lines are out of the tree at HEAD. What is not done is the guard that would have stopped them getting in, and the integration habit of naming failing tests rather than counting them. Both are what the ticket is actually for.
