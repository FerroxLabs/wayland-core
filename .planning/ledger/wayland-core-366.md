---
issue: 366
repo: FerroxLabs/wayland-core
kind: defect
title: "Container orphan scan is nonce-scoped, so it can never see a leftover from an earlier run"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reachable from an operator-facing surface, not only from a nonce-scoped path"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c2
    text: "The nonce-scoped scan_orphans(nonce) contract is left intact for cancel(); the unscoped scan is an addition, not a widening, and the moved and unmoved callers are stated"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c3
    text: "An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live registry"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c4
    text: "The conformance check at conformance.rs:340 either gains an arm that plants a labelled container and requires the scan to FIND it, or its limits are stated so it is not read as orphan-scan coverage"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c5
    text: "A regression test plants a labelled leftover under a nonce the running process has never used and asserts the unscoped scan reports it, creating and cleaning up the leftover itself"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.
