---
issue: 374
repo: FerroxLabs/wayland-core
kind: defect
title: "core#238 c6's evidence test cannot establish its premise on Windows: ENOTDIR provocation maps to NotFound, and it hard-fails the nightly soak"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The test has a Windows arm that produces a genuinely non-NotFound fs::metadata failure -- an over-long path, a path under a directory the process cannot traverse, or an ERROR_INVALID_NAME spelling"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c2
    text: "The existing ENOTDIR provocation is kept for Unix"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c3
    text: "The premise assertion is NOT weakened to make it pass: a test that stops checking its premise is the vacuity this one was written to avoid"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.
