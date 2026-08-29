---
issue: 370
repo: FerroxLabs/wayland-core
kind: defect
title: "Edit-vs-save loses data on Windows: 7 of 169 interleavings lost at retries=0, and every ReplaceFileW failure degrades silently to the racy fallback"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The two named arms pass at retries=0 over N >= 20 on Windows, OR they are gated with the measured Windows rate recorded and a separate arm grading whatever weaker guarantee Windows is declared to give"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c2
    text: "A negative control proves the silent-degrade path is observable when it fires"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.
