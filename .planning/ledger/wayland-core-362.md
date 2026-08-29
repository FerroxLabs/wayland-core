---
issue: 362
repo: FerroxLabs/wayland-core
kind: defect
title: "bwrap backend: sandbox process-tree ownership races with ENOENT, and a containment test retries into a pass having never run its probe"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The ENOENT is traced to a named path and a named window: what is resolved, what opens it, and what removes it in between"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c2
    text: "It is established BY MEASUREMENT whether the race reproduces on a plain Linux host with the sandbox enabled, or only under CI's nested-bwrap-in-docker shape; severity follows from this"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c3
    text: "If it reaches a plain host, the ownership acquisition is made race-free and a red arm is quoted verbatim from before the fix"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c4
    text: "bwrap_confines_filesystem_writes_outside_allowlist cannot retry into a pass having never run its probe: a backend that refuses to start the probe fails the run rather than being retried"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
  - id: c5
    text: "Measured at --retries 0 over N >= 20 on the CI image, with the rate recorded"
    state: not-met
    owner: core
    note: "Seeded 2026-08-29 by the 0.13.12 bookkeeping pass: this open issue had NO ledger file, so it was invisible to scripts/check-criteria-ledger.py's coverage check -- the check that exists because an entire tracker once went unseen for a release. The criterion text is the ticket's own acceptance wording. Nothing has been graded against the tree by this pass."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.
